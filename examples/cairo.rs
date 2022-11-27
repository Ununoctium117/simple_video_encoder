//! An example showing how to use simple_video_encoder to encode frames generated with cairo-rs.

use cairo::{Format, ImageSurface, Context};
use simple_video_encoder::SimpleVideoEncoder;

fn main() {
    let mut encoder =
        SimpleVideoEncoder::new("test_cairo.mp4", 256, 256, 30).expect("Failed to create encoder");

    // Generate 100 frames of video
    for i in 0..100 {
        // Note that only Format::Rgb24 and Format::ARgb24 are supported!
        let surface = ImageSurface::create(Format::Rgb24, 256, 256).unwrap();
        let context = Context::new(&surface).unwrap();
        context.scale(1.0, 1.0);

        // Draw a circle whose color and position changes over time
        context.set_source_rgb(1.0 * (i as f64 / 100.0), 1.0 * (100.0 - (i as f64) / 100.0), 0.2);
        context.arc(100.0 + i as f64, 100.0, 25.0, 0.0, std::f64::consts::TAU);
        context.fill().unwrap();

        encoder.append_frame_cairo(&surface).unwrap();
    }

    encoder.finish().unwrap();
}
