//! Diagnostic: capture IR frames, run the SCRFD detector and save what the
//! encoder would see, to check detection range and alignment by eye.
//!
//!   align-probe [DEVICE] [DETECTOR_MODEL] [OUT_DIR] [FRAMES]
//!
//! Writes OUT_DIR/frame-N.png (equalised frame, box and landmarks drawn) and
//! OUT_DIR/crop-N.png (the aligned 112×112 encoder input).
use face_auth_core::capture::Camera;
use face_auth_core::detector::{assess_frame, FaceDetector, FrameQuality};
use face_auth_core::preprocess::{align_face, histogram_equalize};
use image::{GrayImage, Luma};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let device = args.first().map_or("/dev/video2", String::as_str);
    let model = args.get(1).map_or("/usr/local/share/face-auth/det_500m.onnx", String::as_str);
    let out_dir = std::path::PathBuf::from(args.get(2).map_or(".", String::as_str));
    let frames: usize = args.get(3).and_then(|n| n.parse().ok()).unwrap_or(5);

    let t = Instant::now();
    let mut detector = FaceDetector::new(model, 0.5)?;
    println!("detector loaded in {:?}", t.elapsed());
    // DEVICE may also be a saved greyscale PNG, to re-run a captured frame.
    let saved = device.ends_with(".png").then(|| image::open(device)).transpose()?.map(|i| i.to_luma8());
    let mut camera = if saved.is_none() { Some(Camera::open(device)?) } else { None };

    for n in 0..frames {
        let captured = match (&saved, camera.as_mut()) {
            (Some(img), _) => Ok(face_auth_core::capture::IrFrame {
                data: img.pixels().map(|p| p.0[0] as u16 * 257).collect(),
                width: img.width(),
                height: img.height(),
            }),
            (None, Some(cam)) => cam.capture_illuminated_frame(2000),
            (None, None) => unreachable!(),
        };
        let mut frame = match captured {
            Ok(f) => f,
            Err(e) => {
                println!("{n}: capture failed: {e}");
                continue;
            }
        };
        let quality = assess_frame(&frame);
        if quality != FrameQuality::Ok {
            println!("{n}: skipped, {quality}");
            continue;
        }
        histogram_equalize(&mut frame);

        let t = Instant::now();
        let found = detector.detect(&frame)?;
        let elapsed = t.elapsed();

        let mut img = GrayImage::from_fn(frame.width, frame.height, |x, y| {
            Luma([(frame.data[(y * frame.width + x) as usize] / 257) as u8])
        });

        let Some(face) = found else {
            println!("{n}: no face ({elapsed:?})");
            img.save(out_dir.join(format!("frame-{n}.png")))?;
            continue;
        };

        let [x1, y1, x2, y2] = face.bbox;
        println!(
            "{n}: face score {:.2}, box {:.0}x{:.0} at ({x1:.0},{y1:.0}) ({elapsed:?})",
            face.score,
            x2 - x1,
            y2 - y1
        );

        // Box outline and a small cross per landmark.
        let mut put = |x: f32, y: f32| {
            if x >= 0.0 && y >= 0.0 && (x as u32) < img.width() && (y as u32) < img.height() {
                img.put_pixel(x as u32, y as u32, Luma([255]));
            }
        };
        for i in 0..=100 {
            let f = i as f32 / 100.0;
            put(x1 + (x2 - x1) * f, y1);
            put(x1 + (x2 - x1) * f, y2);
            put(x1, y1 + (y2 - y1) * f);
            put(x2, y1 + (y2 - y1) * f);
        }
        for [lx, ly] in face.landmarks {
            for d in -3..=3 {
                put(lx + d as f32, ly);
                put(lx, ly + d as f32);
            }
        }
        img.save(out_dir.join(format!("frame-{n}.png")))?;

        let crop = align_face(&frame, &face.landmarks)?;
        let crop_img = GrayImage::from_fn(112, 112, |x, y| {
            Luma([((crop[[0, y as usize, x as usize]] * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8])
        });
        crop_img.save(out_dir.join(format!("crop-{n}.png")))?;
    }
    Ok(())
}
