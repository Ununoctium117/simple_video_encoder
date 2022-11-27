use simple_video_encoder::SimpleVideoEncoder;

fn main() {
    let mut encoder =
        SimpleVideoEncoder::new("test_image.mp4", 256, 256, 30).expect("Failed to create encoder");
    let mut frame = encoder.new_frame().unwrap();

    let mut image_buf = image::ImageBuffer::new(256, 256);
    // Generate 100 frames of video
    for i in 0..100 {
        for (x, y, pixel) in image_buf.enumerate_pixels_mut() {
            let r = (0.3 * x as f32) as u8;
            let g = (0.3 * i as f32) as u8;
            let b = (0.3 * y as f32) as u8;
            *pixel = image::Rgb([r, g, b]);
        }

        frame.fill_from_image_rgb(&image_buf).unwrap();
        encoder.append_frame(&mut frame).unwrap();
    }

    encoder.finish().unwrap();
}
