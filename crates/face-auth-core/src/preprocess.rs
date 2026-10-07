use crate::capture::IrFrame;
use image::{ImageBuffer, Luma};
use tract_onnx::prelude::tract_ndarray::Array3;

const ENCODER_SIZE: usize = 112;

/// Wrap a captured frame as a 16-bit greyscale image.
///
/// Shared by the detector and encoder paths so the geometry check lives in
/// one place: `ImageBuffer::from_fn` indexes `width * height` pixels, and a
/// frame shorter than that would otherwise be silently zero-padded into a
/// picture that is part real and part black.
pub fn frame_to_luma16(frame: &IrFrame) -> anyhow::Result<ImageBuffer<Luma<u16>, Vec<u16>>> {
    let width = frame.width;
    let height = frame.height;
    anyhow::ensure!(width > 0 && height > 0, "frame has zero extent");

    let expected = width as usize * height as usize;
    anyhow::ensure!(
        frame.data.len() >= expected,
        "frame holds {} samples, expected {} for {}x{}",
        frame.data.len(),
        expected,
        width,
        height
    );

    Ok(ImageBuffer::from_fn(width, height, |x, y| {
        Luma([frame.data[(y * width + x) as usize]])
    }))
}

/// Where the standard ArcFace 112×112 crop puts the left eye, right eye, nose
/// tip and left and right mouth corners. The encoder was trained on faces
/// warped onto exactly these points.
pub const ARCFACE_LANDMARKS: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// Least-squares similarity transform (rotation, uniform scale, translation)
/// taking `src` onto `dst`, as `(a, b, tx, ty)` for
///
/// ```text
/// x' = a·x − b·y + tx
/// y' = b·x + a·y + ty
/// ```
///
/// This is the closed form of Umeyama's method in two dimensions, which is
/// what InsightFace uses to align faces for these models.
pub fn estimate_similarity(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> (f32, f32, f32, f32) {
    let n = src.len() as f64;
    let mean = |pts: &[[f32; 2]; 5]| {
        let (sx, sy) = pts
            .iter()
            .fold((0.0f64, 0.0f64), |(x, y), p| (x + p[0] as f64, y + p[1] as f64));
        (sx / n, sy / n)
    };
    let (msx, msy) = mean(src);
    let (mdx, mdy) = mean(dst);

    let (mut num_a, mut num_b, mut den) = (0.0f64, 0.0f64, 0.0f64);
    for (s, d) in src.iter().zip(dst) {
        let (sx, sy) = (s[0] as f64 - msx, s[1] as f64 - msy);
        let (dx, dy) = (d[0] as f64 - mdx, d[1] as f64 - mdy);
        num_a += sx * dx + sy * dy;
        num_b += sx * dy - sy * dx;
        den += sx * sx + sy * sy;
    }
    let (a, b) = if den > 0.0 { (num_a / den, num_b / den) } else { (0.0, 0.0) };
    let tx = mdx - (a * msx - b * msy);
    let ty = mdy - (b * msx + a * msy);
    (a as f32, b as f32, tx as f32, ty as f32)
}

/// Warp the face at `landmarks` (frame coordinates, in [`ARCFACE_LANDMARKS`]
/// order) into the encoder's aligned, normalised `[3, 112, 112]` input.
///
/// The arithmetic here defines what an enrolled embedding means; changing it
/// invalidates every template already on disk (see `EMBEDDING_VERSION`).
pub fn align_face(frame: &IrFrame, landmarks: &[[f32; 2]; 5]) -> anyhow::Result<Array3<f32>> {
    let img = frame_to_luma16(frame)?;
    let (width, height) = img.dimensions();

    let (a, b, tx, ty) = estimate_similarity(landmarks, &ARCFACE_LANDMARKS);
    let det = a * a + b * b;
    anyhow::ensure!(det.is_finite() && det > 1e-12, "degenerate face landmarks");

    let sample = |x: i64, y: i64| -> f32 {
        if x < 0 || y < 0 || x >= width as i64 || y >= height as i64 {
            0.0 // outside the frame: black, as warpAffine pads
        } else {
            img.get_pixel(x as u32, y as u32).0[0] as f32
        }
    };

    let mut array = Array3::<f32>::zeros((3, ENCODER_SIZE, ENCODER_SIZE));
    for v in 0..ENCODER_SIZE {
        for u in 0..ENCODER_SIZE {
            // Inverse map from crop pixel to frame position, then bilinear.
            let du = u as f32 - tx;
            let dv = v as f32 - ty;
            let x = (a * du + b * dv) / det;
            let y = (-b * du + a * dv) / det;

            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = (x - x0, y - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let top = sample(x0, y0) * (1.0 - fx) + sample(x0 + 1, y0) * fx;
            let bottom = sample(x0, y0 + 1) * (1.0 - fx) + sample(x0 + 1, y0 + 1) * fx;
            let pixel = (top * (1.0 - fy) + bottom * fy) / 65535.0;

            let normalized = (pixel - 0.5) / 0.5;
            for c in 0..3usize {
                array[[c, v, u]] = normalized;
            }
        }
    }

    Ok(array)
}

/// Default CLAHE parameters. `CLIP_LIMIT` follows OpenCV's convention: the
/// per-bin ceiling is `clip * tile_pixels / 256`, with the clipped mass
/// redistributed. Howdy uses 2.0; 3.0 measured better on this sensor's frames
/// without visibly amplifying noise.
pub const CLAHE_CLIP_LIMIT: f32 = 3.0;
pub const CLAHE_TILES: u32 = 8;

/// Contrast-limited adaptive histogram equalisation.
///
/// Replaces the global equalisation this used to do. Global equalisation maps
/// one CDF over the whole frame, so a dark IR frame — where nearly all samples
/// sit in a narrow band — gets that band stretched across the full range,
/// turning sensor noise into hard posterised contours. Measured on this
/// camera, the face detector scored ~0.11 on globally-equalised frames (below
/// its 0.5 threshold, indistinguishable from an empty room) and 0.60-0.99 on
/// the same frames under CLAHE.
///
/// CLAHE instead equalises per tile with a ceiling on how much any one
/// intensity may be amplified, then bilinearly interpolates between
/// neighbouring tiles' mappings so no tile seams appear.
///
/// Samples are u16 carrying 8-bit data widened by 257 (see `capture_frame`),
/// so 256 bins are exact here.
pub fn clahe_equalize(frame: &mut IrFrame, clip_limit: f32, tiles: u32) {
    let (w, h) = (frame.width as usize, frame.height as usize);
    if frame.data.is_empty() || w == 0 || h == 0 || frame.data.len() < w * h {
        return;
    }
    let tiles = tiles.max(1) as usize;
    let tile_w = w.div_ceil(tiles);
    let tile_h = h.div_ceil(tiles);

    // One 256-entry lookup table per tile.
    let mut luts = vec![[0u8; 256]; tiles * tiles];
    for ty in 0..tiles {
        for tx in 0..tiles {
            let x0 = tx * tile_w;
            let y0 = ty * tile_h;
            let x1 = (x0 + tile_w).min(w);
            let y1 = (y0 + tile_h).min(h);
            if x0 >= x1 || y0 >= y1 {
                continue;
            }

            let mut hist = [0u32; 256];
            for y in y0..y1 {
                for x in x0..x1 {
                    hist[(frame.data[y * w + x] >> 8) as usize] += 1;
                }
            }

            // Clip, then hand the excess back evenly. This is what bounds the
            // contrast gain and stops noise being amplified without limit.
            let count = ((x1 - x0) * (y1 - y0)) as f32;
            let limit = ((clip_limit * count) / 256.0).max(1.0) as u32;
            let mut excess: u32 = 0;
            for b in hist.iter_mut() {
                if *b > limit {
                    excess += *b - limit;
                    *b = limit;
                }
            }
            let share = excess / 256;
            let mut remainder = excess % 256;
            for b in hist.iter_mut() {
                *b += share;
                if remainder > 0 {
                    *b += 1;
                    remainder -= 1;
                }
            }

            let lut = &mut luts[ty * tiles + tx];
            let total = count.max(1.0);
            let mut cumulative = 0u32;
            for (i, &b) in hist.iter().enumerate() {
                cumulative += b;
                lut[i] = ((cumulative as f32 / total) * 255.0).clamp(0.0, 255.0) as u8;
            }
        }
    }

    // Bilinear blend between the four nearest tile centres.
    for y in 0..h {
        let gy = ((y as f32 - tile_h as f32 * 0.5) / tile_h as f32).max(0.0);
        let ty0 = (gy as usize).min(tiles - 1);
        let ty1 = (ty0 + 1).min(tiles - 1);
        let fy = gy - ty0 as f32;

        for x in 0..w {
            let gx = ((x as f32 - tile_w as f32 * 0.5) / tile_w as f32).max(0.0);
            let tx0 = (gx as usize).min(tiles - 1);
            let tx1 = (tx0 + 1).min(tiles - 1);
            let fx = gx - tx0 as f32;

            let v = (frame.data[y * w + x] >> 8) as usize;
            let tl = luts[ty0 * tiles + tx0][v] as f32;
            let tr = luts[ty0 * tiles + tx1][v] as f32;
            let bl = luts[ty1 * tiles + tx0][v] as f32;
            let br = luts[ty1 * tiles + tx1][v] as f32;

            let top = tl + (tr - tl) * fx;
            let bottom = bl + (br - bl) * fx;
            let out = (top + (bottom - top) * fy).clamp(0.0, 255.0) as u16;
            frame.data[y * w + x] = out * 257;
        }
    }
}

/// Equalise a frame for detection and encoding, using the project defaults.
pub fn histogram_equalize(frame: &mut IrFrame) {
    clahe_equalize(frame, CLAHE_CLIP_LIMIT, CLAHE_TILES);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(data: Vec<u16>, width: u32, height: u32) -> IrFrame {
        IrFrame { data, width, height }
    }

    #[test]
    fn rejects_frame_shorter_than_its_geometry() {
        // Previously padded with zeros, producing a half-black image that the
        // encoder would happily turn into an embedding.
        let f = frame(vec![0; 10], 32, 32);
        assert!(frame_to_luma16(&f).is_err());
        assert!(align_face(&f, &ARCFACE_LANDMARKS).is_err());
    }

    #[test]
    fn rejects_zero_extent_frame() {
        assert!(frame_to_luma16(&frame(vec![], 0, 0)).is_err());
    }

    #[test]
    fn accepts_exactly_sized_frame() {
        let f = frame(vec![1234; 32 * 32], 32, 32);
        let img = frame_to_luma16(&f).unwrap();
        assert_eq!(img.dimensions(), (32, 32));
    }

    #[test]
    fn accepts_frame_with_trailing_padding() {
        // Some drivers report bytesused beyond the visible image.
        let f = frame(vec![7; 32 * 32 + 64], 32, 32);
        assert!(frame_to_luma16(&f).is_ok());
    }

    #[test]
    fn similarity_recovers_a_known_transform() {
        // Rotate 30°, scale 2, translate (5, -3).
        let (sin, cos) = 30f32.to_radians().sin_cos();
        let (a, b, tx, ty) = (2.0 * cos, 2.0 * sin, 5.0, -3.0);
        let src = ARCFACE_LANDMARKS;
        let dst = src.map(|[x, y]| [a * x - b * y + tx, b * x + a * y + ty]);
        let (ea, eb, etx, ety) = estimate_similarity(&src, &dst);
        for (got, want) in [(ea, a), (eb, b), (etx, tx), (ety, ty)] {
            assert!((got - want).abs() < 1e-3, "got {got}, want {want}");
        }
    }

    #[test]
    fn aligned_face_at_reference_position_is_the_frame_itself() {
        // A 112×112 frame whose landmarks already sit on the ArcFace points
        // aligns to an identity warp: the crop is the frame, normalised.
        let f = frame((0..112 * 112).map(|i| ((i % 251) as u16) * 257).collect(), 112, 112);
        let arr = align_face(&f, &ARCFACE_LANDMARKS).unwrap();
        assert_eq!(arr.shape(), &[3, ENCODER_SIZE, ENCODER_SIZE]);
        for (y, x) in [(0, 0), (40, 70), (111, 111)] {
            let want = (f.data[y * 112 + x] as f32 / 65535.0 - 0.5) / 0.5;
            assert!((arr[[0, y, x]] - want).abs() < 1e-4, "pixel ({x}, {y})");
            assert_eq!(arr[[0, y, x]], arr[[2, y, x]]);
        }
    }

    #[test]
    fn aligned_face_follows_the_landmarks() {
        // The face is twice the reference size and offset by (100, 50): a
        // bright patch around the frame-space nose tip must land on the
        // crop's nose tip.
        let (w, h) = (400u32, 300u32);
        let landmarks = ARCFACE_LANDMARKS.map(|[x, y]| [2.0 * x + 100.0, 2.0 * y + 50.0]);
        let [nx, ny] = landmarks[2];
        let data = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                if (x - nx).abs() < 6.0 && (y - ny).abs() < 6.0 { 65535 } else { 0 }
            })
            .collect();
        let arr = align_face(&frame(data, w, h), &landmarks).unwrap();
        let [cx, cy] = ARCFACE_LANDMARKS[2];
        assert!(arr[[0, cy as usize, cx as usize]] > 0.9, "nose tip should be bright");
        assert!(arr[[0, 10, 10]] < -0.9, "corner should be dark");
    }

    #[test]
    fn degenerate_landmarks_are_rejected() {
        let f = frame(vec![0; 64 * 64], 64, 64);
        assert!(align_face(&f, &[[10.0, 10.0]; 5]).is_err());
    }

    #[test]
    fn clahe_expands_a_low_contrast_frame() {
        // A dark, narrow-range frame — what an unlit-ish IR capture looks like.
        let data: Vec<u16> = (0..64 * 64).map(|i| ((i % 20) as u16 + 20) * 257).collect();
        let mut f = frame(data, 64, 64);
        let before = spread(&f);
        histogram_equalize(&mut f);
        assert!(spread(&f) > before, "CLAHE should widen the tonal range");
        assert!(f.data.iter().all(|&v| v % 257 == 0), "output stays 8-bit widened");
    }

    #[test]
    fn clahe_leaves_a_flat_frame_flat() {
        // Uniform input has no contrast to recover; it must not explode into
        // noise, which is precisely what the clip limit is for.
        let mut f = frame(vec![128 * 257; 64 * 64], 64, 64);
        histogram_equalize(&mut f);
        let first = f.data[0];
        assert!(f.data.iter().all(|&v| v == first));
    }

    #[test]
    fn clahe_handles_degenerate_frames() {
        let mut empty = frame(vec![], 0, 0);
        histogram_equalize(&mut empty);
        assert!(empty.data.is_empty());

        // Short buffer must be left alone rather than indexed out of bounds.
        let mut short = frame(vec![100; 10], 32, 32);
        histogram_equalize(&mut short);
        assert_eq!(short.data.len(), 10);
    }

    fn spread(f: &IrFrame) -> u16 {
        let max = f.data.iter().copied().max().unwrap_or(0);
        let min = f.data.iter().copied().min().unwrap_or(0);
        max - min
    }
}
