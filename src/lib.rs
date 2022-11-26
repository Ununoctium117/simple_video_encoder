//! Provides a simple and easy-to-use video encoder, which allows turning a series
//! of images into a video without having to worry about the details.

#![deny(missing_docs, unconditional_panic)]

use std::{
    error::Error,
    ffi::CStr,
    path::{Path, PathBuf},
};

use ffmpeg_sys_next::{av_make_error_string, AVCodecID, AVPixelFormat, AV_ERROR_MAX_STRING_SIZE};
use frame::Frame;

use crate::output::{OutputFormatContext, OutputStream};

mod frame;
mod output;

fn make_av_error(action: impl Into<String>, err: i32) -> Box<dyn Error> {
    let mut buffer = [0u8; AV_ERROR_MAX_STRING_SIZE];
    unsafe {
        av_make_error_string(buffer.as_mut_ptr() as *mut i8, buffer.len(), err);
    }

    let (idx, _) = buffer
        .iter()
        .enumerate()
        .find(|(_, x)| **x == 0)
        .expect("av_make_error_string returned string without null terminator");

    let str = CStr::from_bytes_with_nul(&buffer[..idx]).unwrap();
    format!(
        "Error {}: {}",
        action.into(),
        str.to_str()
            .expect("av_make_error_string returned invalid UTF-8")
    )
    .into()
}

/// The possible presets for libx264. These are listed in descending order of speed.
/// See https://trac.ffmpeg.org/wiki/Encode/H.264 for more information.
pub enum X264Preset {
    /// The fastest preset
    UltraFast,
    #[allow(missing_docs)]
    SuperFast,
    #[allow(missing_docs)]
    VeryFast,
    #[allow(missing_docs)]
    Faster,
    #[allow(missing_docs)]
    Fast,
    /// The default preset
    Medium,
    #[allow(missing_docs)]
    Slow,
    #[allow(missing_docs)]
    Slower,
    /// The slowest preset
    VerySlow,
}
impl X264Preset {
    fn as_bytes_with_nul(&self) -> *const i8 {
        match self {
            X264Preset::UltraFast => "ultrafast\0",
            X264Preset::SuperFast => "superfast\0",
            X264Preset::VeryFast => "veryfast\0",
            X264Preset::Faster => "faster\0",
            X264Preset::Fast => "fast\0",
            X264Preset::Medium => "medium\0",
            X264Preset::Slow => "slow\0",
            X264Preset::Slower => "slower\0",
            X264Preset::VerySlow => "veryslow\0",
        }
        .as_ptr() as *const i8
    }
}

/// Helper to build a SimpleVideoEncoder, allowing you to specify more options.
pub struct SimpleVideoEncoderBuilder {
    filename: PathBuf,
    width: i32,
    height: i32,
    framerate: i32,

    crf: Option<i64>,
    bitrate: Option<i64>,
    gop_size: Option<i32>,
    preset: Option<X264Preset>,
}
impl SimpleVideoEncoderBuilder {
    fn new<P: AsRef<Path>>(filename: P, width: i32, height: i32, framerate: i32) -> Self {
        Self {
            filename: filename.as_ref().to_path_buf(),
            width,
            height,
            framerate,

            crf: None,
            bitrate: None,
            gop_size: None,
            preset: None,
        }
    }

    /// Sets the CRF, the constant-rate function. See https://trac.ffmpeg.org/wiki/Encode/H.264 for more details.
    ///
    /// The range of values is 0-51; lower values produce higher-quality output at the expense of higher filesize.
    /// Values around 17-18 should be visually lossless.
    ///
    /// If you specify this, the bitrate setting is ignored.
    pub fn crf(&mut self, crf: i64) -> &mut Self {
        self.crf = Some(crf);
        self
    }

    /// Set the preset, a collection of options that allow trading off encoding speed for output file size and vice versa.
    /// If you combine this with setting the CRF, a slower preset will improve your bitrate.
    /// If you combine this with setting the bitrate, a slower preset will achieve better quality.
    /// See https://trac.ffmpeg.org/wiki/Encode/H.264 for more information.
    pub fn preset(&mut self, preset: X264Preset) -> &mut Self {
        self.preset = Some(preset);
        self
    }

    /// Sets the target bitrate. It's preferred to use the CRF settings, and setting a CRF value
    /// will mean that this setting has no effect.
    /// Bitrate is `output filesize / duration` and is measured in bits/second. Compression will
    /// not achieve this bitrate exactly, but will target it.
    pub fn bitrate(&mut self, bitrate: i64) -> &mut Self {
        self.bitrate = Some(bitrate);
        self
    }

    /// Sets the group-of-pictures size, the maximum number of frames between I-frames (keyframes).
    /// Higher values will result in smaller file sizes, but most video players can only seek to I-frames,
    /// so setting this to a large value may hurt seekability.
    pub fn set_gop_size(&mut self, gop_size: i32) -> &mut Self {
        self.gop_size = Some(gop_size);
        self
    }

    /// Produce a SimpleVideoEncoder using the specified settings.
    pub fn build(self) -> Result<SimpleVideoEncoder, Box<dyn Error>> {
        let mut format_context = OutputFormatContext::new(&self.filename)?;
        let (mut output_stream, codec) = format_context.add_stream(
            AVCodecID::AV_CODEC_ID_H264,
            self.width,
            self.height,
            self.framerate,
            AVPixelFormat::AV_PIX_FMT_YUV420P,
            self.bitrate,
            self.gop_size,
        )?;

        output_stream.open_video(codec, self.preset, self.crf)?;
        format_context.open_file()?;
        format_context.write_header()?;

        Ok(SimpleVideoEncoder {
            temp_rgb_frame: Frame::new(AVPixelFormat::AV_PIX_FMT_RGB24, self.width, self.height)?,
            output_stream,
            format_context,
        })
    }
}

/// A simple video encoder that can accept frames of video and will write them into a video.
pub struct SimpleVideoEncoder {
    temp_rgb_frame: Frame,
    output_stream: OutputStream,
    // Ensure that this is dropped last, since the OutputStream must not outlive it
    format_context: OutputFormatContext,
}
impl SimpleVideoEncoder {
    /// Creates a SimpleVideoEncoder targeting the specified file name with default settings.
    pub fn new<P: AsRef<Path>>(
        filename: P,
        width: i32,
        height: i32,
        framerate: i32,
    ) -> Result<Self, Box<dyn Error>> {
        SimpleVideoEncoderBuilder::new(filename, width, height, framerate).build()
    }

    /// Produces a builder, allowing you to specify additional settings.
    pub fn builder<P: AsRef<Path>>(
        filename: P,
        width: i32,
        height: i32,
        framerate: i32,
    ) -> SimpleVideoEncoderBuilder {
        SimpleVideoEncoderBuilder::new(filename, width, height, framerate)
    }

    /// Finishes encoding the video, flushing everything to disk and tearing down the internals.
    pub fn finish(mut self) -> Result<(), Box<dyn Error>> {
        self.output_stream.finish(&self.format_context)?;
        self.format_context.write_trailer()?;
        Ok(())
    }

    /// Appends a frame to the video, sourcing the data from a Cairo ImageSurface.
    /// Transparency is ignored - but note that Cairo uses premultiplied alpha, so you
    /// may get unexpected results if you provide an image with non-zero alpha values.
    #[cfg(feature = "cairo")]
    pub fn append_frame_cairo(&mut self, data: &cairo::ImageSurface) -> Result<(), Box<dyn Error>> {
        self.temp_rgb_frame.fill_from_cairo_rgb(data)?;
        self.output_stream
            .write_frame(&mut self.temp_rgb_frame, &self.format_context)?;
        Ok(())
    }
}