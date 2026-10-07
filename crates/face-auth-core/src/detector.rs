use tract_onnx::prelude::*;

use crate::capture::IrFrame;

// Frame-quality gates, expressed in 8-bit-equivalent units so the numbers can
// be compared against what `cargo run --example frame-stats` prints.
//
// Measured on the reference ASUS IR sensor:
//   illuminated frames   mean 48-96,  variance  82-317
//   dark (strobe off)    mean 1.8-8,  variance 3.4-39
//
// The old gate was a variance floor of 100_000 in the u16 domain, which is a
// variance of 1.5 in these units — low enough that the dark frames passed it
// and were then histogram-equalised into a grey noise field.
const MIN_FRAME_MEAN_8BIT: f64 = 12.0;
const MIN_FRAME_VARIANCE_8BIT: f64 = 20.0;

/// Samples are u16 but carry 8-bit data widened by 257 (see `capture_frame`).
const U16_PER_8BIT: f64 = 257.0;

/// Why a frame was rejected before inference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrameQuality {
    Ok,
    Empty,
    /// Almost certainly an unlit frame from a strobing IR illuminator.
    TooDark { mean_8bit: f64 },
    /// Uniform field — a covered lens, or a wall.
    TooFlat { variance_8bit: f64 },
}

impl std::fmt::Display for FrameQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameQuality::Ok => write!(f, "ok"),
            FrameQuality::Empty => write!(f, "empty frame"),
            FrameQuality::TooDark { mean_8bit } => write!(
                f,
                "too dark (mean {mean_8bit:.1}/255, need {MIN_FRAME_MEAN_8BIT}) \
                 — unlit frame, or the IR illuminator is not firing"
            ),
            FrameQuality::TooFlat { variance_8bit } => write!(
                f,
                "too flat (variance {variance_8bit:.1}, need {MIN_FRAME_VARIANCE_8BIT}) \
                 — lens covered, or nothing in view"
            ),
        }
    }
}

// SCRFD (InsightFace's detector, `det_500m.onnx` from the buffalo_sc pack).
//
// The input holds a 640×360 IR frame at full resolution, padded at the bottom:
// a distant face is only a few dozen pixels wide, and downscaling the frame
// first is what made the previous 320×240 detector lose it. Both sides must be
// multiples of the largest stride.
const DETECTOR_WIDTH: usize = 640;
const DETECTOR_HEIGHT: usize = 384;
const STRIDES: [usize; 3] = [8, 16, 32];
const ANCHORS_PER_CELL: usize = 2;

/// A detected face, in frame coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub score: f32,
    /// `[x1, y1, x2, y2]`.
    pub bbox: [f32; 4],
    /// Left eye, right eye, nose tip, left and right mouth corners, as
    /// `[x, y]` — the order `preprocess::align_face` expects.
    pub landmarks: [[f32; 2]; 5],
}

impl Detection {
    fn scaled(mut self, factor: f32) -> Self {
        for v in &mut self.bbox {
            *v *= factor;
        }
        for p in &mut self.landmarks {
            p[0] *= factor;
            p[1] *= factor;
        }
        self
    }
}

pub struct FaceDetector {
    model: TypedRunnableModel<TypedModel>,
    threshold: f32,
}

impl FaceDetector {
    pub fn new(model_path: &str, threshold: f32) -> anyhow::Result<Self> {
        if !std::path::Path::new(model_path).exists() {
            anyhow::bail!("face detector model not found at {model_path}");
        }
        let mut proto = onnx().proto_model_for_path(model_path)?;
        use_constant_resize_scales(&mut proto);

        // The input fact is required before optimisation: without a concrete
        // shape the graph stays symbolic and tract cannot lower it.
        let model = onnx()
            .model_for_proto_model(&proto)?
            .with_input_fact(
                0,
                InferenceFact::dt_shape(
                    f32::datum_type(),
                    tvec!(1, 3, DETECTOR_HEIGHT, DETECTOR_WIDTH),
                ),
            )?
            .into_optimized()?
            .into_runnable()?;
        Ok(Self { model, threshold })
    }

    /// The most confident face at or above the threshold, if any.
    pub fn detect(&mut self, frame: &IrFrame) -> anyhow::Result<Option<Detection>> {
        let (input, scale) = preprocess_for_detector(frame)?;
        let mut input = input.into_dyn();
        input.insert_axis_inplace(tract_ndarray::Axis(0));
        let result = self.model.run(tvec!(Tensor::from(input).into_tvalue()))?;

        // Each output is [N, C] (or [1, N, C]): scores (C=1), box distances
        // (C=4) or landmark offsets (C=10), N = cells × anchors per stride.
        let mut outputs = Vec::with_capacity(result.len());
        for tensor in result.iter() {
            let view = tensor.to_array_view::<f32>()?;
            let shape = view.shape();
            let (rows, cols) = match *shape {
                [n, c] | [1, n, c] => (n, c),
                _ => anyhow::bail!("unexpected detector output shape {shape:?}; is this an SCRFD model?"),
            };
            outputs.push((rows, cols, view.iter().copied().collect::<Vec<f32>>()));
        }

        let best = decode_best(&outputs, self.threshold)?;
        Ok(best.map(|d| d.scaled(1.0 / scale)))
    }
}

/// SCRFD's two FPN upsampling `Resize` nodes compute their output size at run
/// time (Shape → Slice → Concat). tract 0.21 evaluates that wrongly — unoptimised
/// it fails with "Can not broadcast 40 against 20"; optimised it silently
/// returns garbage scores. With both input sides multiples of 32 each one is an
/// exact 2× nearest upsample, so give them constant scales instead. Checked
/// bit-identical against the original graph in onnxruntime.
fn use_constant_resize_scales(proto: &mut tract_onnx::pb::ModelProto) {
    const SCALES: &str = "face_auth_resize_scales";
    let Some(graph) = proto.graph.as_mut() else {
        return;
    };
    let constants: std::collections::HashSet<String> =
        graph.initializer.iter().map(|t| t.name.clone()).collect();

    let mut patched = 0;
    for node in &mut graph.node {
        // Inputs are (X, roi, scales, sizes); only rewrite a computed `sizes`.
        if node.op_type == "Resize" && node.input.len() == 4 && !constants.contains(&node.input[3]) {
            node.input[2] = SCALES.to_string();
            node.input.truncate(3);
            patched += 1;
        }
    }
    if patched > 0 {
        graph.initializer.push(tract_onnx::pb::TensorProto {
            name: SCALES.to_string(),
            dims: vec![4],
            data_type: 1, // FLOAT
            float_data: vec![1.0, 1.0, 2.0, 2.0],
            ..Default::default()
        });
    }
}

/// Decode SCRFD's anchor-free outputs and keep the single best face.
///
/// No NMS: authentication only ever wants one face, and overlapping
/// detections of the same face differ by less than the alignment tolerates.
fn decode_best(outputs: &[(usize, usize, Vec<f32>)], threshold: f32) -> anyhow::Result<Option<Detection>> {
    let mut best: Option<Detection> = None;

    for stride in STRIDES {
        let cells_w = DETECTOR_WIDTH / stride;
        let cells_h = DETECTOR_HEIGHT / stride;
        let rows = cells_w * cells_h * ANCHORS_PER_CELL;
        let find = |cols: usize| {
            outputs
                .iter()
                .find(|(r, c, d)| *r == rows && *c == cols && d.len() == rows * cols)
                .map(|(_, _, d)| d)
        };
        let (Some(scores), Some(boxes), Some(kps)) = (find(1), find(4), find(10)) else {
            anyhow::bail!(
                "detector outputs lack the {rows}-row score/box/landmark set for stride {stride}; \
                 is this an SCRFD model?"
            );
        };

        let s = stride as f32;
        for (i, &score) in scores.iter().enumerate() {
            if !score.is_finite() || score < threshold {
                continue;
            }
            if best.as_ref().is_some_and(|b| b.score >= score) {
                continue;
            }
            // Anchors are laid out row-major over the grid, two per cell.
            let cell = i / ANCHORS_PER_CELL;
            let cx = ((cell % cells_w) * stride) as f32;
            let cy = ((cell / cells_w) * stride) as f32;

            let d = &boxes[i * 4..i * 4 + 4];
            let k = &kps[i * 10..i * 10 + 10];
            let mut landmarks = [[0.0f32; 2]; 5];
            for (j, p) in landmarks.iter_mut().enumerate() {
                *p = [cx + k[2 * j] * s, cy + k[2 * j + 1] * s];
            }
            best = Some(Detection {
                score,
                bbox: [cx - d[0] * s, cy - d[1] * s, cx + d[2] * s, cy + d[3] * s],
                landmarks,
            });
        }
    }

    Ok(best)
}

/// Place the frame top-left in the detector input, scaled down only if it does
/// not fit, normalised as SCRFD was trained: `(p - 127.5) / 128`, grey copied
/// to all three channels, padding black. Returns the input and the scale used.
fn preprocess_for_detector(frame: &IrFrame) -> anyhow::Result<(tract_ndarray::Array3<f32>, f32)> {
    let img = crate::preprocess::frame_to_luma16(frame)?;
    let (w, h) = img.dimensions();
    let scale = (DETECTOR_WIDTH as f32 / w as f32)
        .min(DETECTOR_HEIGHT as f32 / h as f32)
        .min(1.0);
    let img = if scale < 1.0 {
        let nw = ((w as f32 * scale).floor() as u32).max(1);
        let nh = ((h as f32 * scale).floor() as u32).max(1);
        image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
    } else {
        img
    };

    let normalize = |v8: f32| (v8 - 127.5) / 128.0;
    let mut array = tract_ndarray::Array3::<f32>::from_elem(
        (3, DETECTOR_HEIGHT, DETECTOR_WIDTH),
        normalize(0.0),
    );
    for (x, y, pixel) in img.enumerate_pixels() {
        let v = normalize(pixel.0[0] as f32 / 257.0);
        for c in 0..3usize {
            array[[c, y as usize, x as usize]] = v;
        }
    }

    Ok((array, scale))
}

/// Cheap "is this frame worth running inference on" check.
///
/// Accumulates in f64: a 640x400 frame sums ~256k terms of up to 4.3e9, well
/// past what f32's ~7 significant digits can carry.
pub fn assess_frame(frame: &IrFrame) -> FrameQuality {
    if frame.data.is_empty() {
        return FrameQuality::Empty;
    }
    let len = frame.data.len() as f64;
    let mean = frame.data.iter().map(|&v| v as f64).sum::<f64>() / len;
    let variance: f64 = frame
        .data
        .iter()
        .map(|&v| {
            let d = v as f64 - mean;
            d * d
        })
        .sum::<f64>()
        / len;

    let mean_8bit = mean / U16_PER_8BIT;
    let variance_8bit = variance / (U16_PER_8BIT * U16_PER_8BIT);

    if mean_8bit < MIN_FRAME_MEAN_8BIT {
        return FrameQuality::TooDark { mean_8bit };
    }
    if variance_8bit < MIN_FRAME_VARIANCE_8BIT {
        return FrameQuality::TooFlat { variance_8bit };
    }
    FrameQuality::Ok
}

pub fn raw_frame_has_content(frame: &IrFrame) -> bool {
    assess_frame(frame) == FrameQuality::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SCRFD-shaped outputs, all zero, with one face planted at `index` of
    /// the given stride.
    fn outputs_with_face(stride: usize, index: usize, score: f32) -> Vec<(usize, usize, Vec<f32>)> {
        let mut out = Vec::new();
        for s in STRIDES {
            let rows = (DETECTOR_WIDTH / s) * (DETECTOR_HEIGHT / s) * ANCHORS_PER_CELL;
            let mut scores = vec![0.0; rows];
            let mut boxes = vec![0.0; rows * 4];
            let mut kps = vec![0.0; rows * 10];
            if s == stride {
                scores[index] = score;
                boxes[index * 4..index * 4 + 4].copy_from_slice(&[1.0, 2.0, 3.0, 4.0]);
                for j in 0..10 {
                    kps[index * 10 + j] = j as f32 * 0.1;
                }
            }
            out.push((rows, 1, scores));
            out.push((rows, 4, boxes));
            out.push((rows, 10, kps));
        }
        out
    }

    #[test]
    fn decodes_a_planted_face() {
        // Stride 16, grid 40 wide: anchor 2×(3·40 + 5) + 1 is cell (5, 3).
        let index = 2 * (3 * 40 + 5) + 1;
        let face = decode_best(&outputs_with_face(16, index, 0.9), 0.5).unwrap().unwrap();
        let (cx, cy) = (80.0, 48.0);
        assert_eq!(face.score, 0.9);
        assert_eq!(face.bbox, [cx - 16.0, cy - 32.0, cx + 48.0, cy + 64.0]);
        assert_eq!(face.landmarks[0], [cx, cy + 1.6]);
        assert!((face.landmarks[4][0] - (cx + 0.8 * 16.0)).abs() < 1e-4);
    }

    #[test]
    fn ignores_faces_below_threshold() {
        assert!(decode_best(&outputs_with_face(8, 10, 0.3), 0.5).unwrap().is_none());
    }

    #[test]
    fn rejects_outputs_from_a_different_model() {
        // The old detector's [1, N, 2] scores have no landmark outputs.
        let foreign = vec![(4420, 2, vec![0.0; 8840])];
        assert!(decode_best(&foreign, 0.5).is_err());
    }

    #[test]
    fn scaling_maps_back_to_frame_coordinates() {
        let d = Detection { score: 1.0, bbox: [10.0, 20.0, 30.0, 40.0], landmarks: [[2.0, 4.0]; 5] };
        let d = d.scaled(2.0);
        assert_eq!(d.bbox, [20.0, 40.0, 60.0, 80.0]);
        assert_eq!(d.landmarks[3], [4.0, 8.0]);
    }

    /// Build a frame from 8-bit values, widened the way `capture_frame` does.
    fn frame8(values: Vec<u8>, width: u32, height: u32) -> IrFrame {
        IrFrame {
            data: values.into_iter().map(|v| v as u16 * 257).collect(),
            width,
            height,
        }
    }

    #[test]
    fn empty_frame_is_rejected() {
        assert_eq!(assess_frame(&frame8(vec![], 0, 0)), FrameQuality::Empty);
    }

    #[test]
    fn covered_lens_is_too_flat() {
        // Uniform mid-grey: bright enough, but no structure at all.
        let q = assess_frame(&frame8(vec![128; 1024], 32, 32));
        assert!(matches!(q, FrameQuality::TooFlat { .. }), "got {q:?}");
    }

    #[test]
    fn unlit_strobe_frame_is_too_dark() {
        // Modelled on a real unlit frame from the reference sensor, which
        // measured mean 8.0 and variance 38 in 8-bit units. Values 0..=16 give
        // mean 8.0, variance 24 — dark, but with *more* than enough variance to
        // clear the old variance-only gate, which is exactly why those frames
        // reached histogram equalisation and became a grey noise field.
        let data: Vec<u8> = (0..1024).map(|i| (i % 17) as u8).collect();
        let q = assess_frame(&frame8(data, 32, 32));
        assert!(matches!(q, FrameQuality::TooDark { .. }), "got {q:?}");

        // The old gate: variance > 100_000 in the u16 domain. Confirm this
        // frame would have sailed through it, so the test documents the bug.
        let frame = frame8((0..1024).map(|i| (i % 17) as u8).collect(), 32, 32);
        let len = frame.data.len() as f64;
        let mean = frame.data.iter().map(|&v| v as f64).sum::<f64>() / len;
        let var_u16 =
            frame.data.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / len;
        assert!(var_u16 > 100_000.0, "u16 variance was {var_u16}");
    }

    #[test]
    fn illuminated_frame_is_accepted() {
        // Mean ~64, plenty of structure — like a real lit frame.
        let data: Vec<u8> = (0..1024).map(|i| (i % 128) as u8).collect();
        assert_eq!(assess_frame(&frame8(data, 32, 32)), FrameQuality::Ok);
        assert!(raw_frame_has_content(&frame8(
            (0..1024).map(|i| (i % 128) as u8).collect(),
            32,
            32
        )));
    }

    #[test]
    fn brightness_alone_does_not_pass() {
        // A bright but featureless frame must still be rejected.
        let q = assess_frame(&frame8(vec![200; 1024], 32, 32));
        assert!(matches!(q, FrameQuality::TooFlat { .. }), "got {q:?}");
    }
}
