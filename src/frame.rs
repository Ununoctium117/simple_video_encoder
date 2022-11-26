use std::{error::Error, ptr::NonNull};

use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_free, av_frame_get_buffer, av_frame_make_writable, AVFrame,
    AVPixelFormat,
};

use crate::make_av_error;

pub(crate) struct Frame {
    frame: NonNull<AVFrame>,
}
impl Frame {
    pub fn new(fmt: AVPixelFormat, width: i32, height: i32) -> Result<Self, Box<dyn Error>> {
        let Some(mut frame) = NonNull::new(unsafe { av_frame_alloc() }) else {
            return Err("Error allocating AVFrame".into());
        };

        unsafe {
            frame.as_mut().format = fmt as i32;
            frame.as_mut().width = width;
            frame.as_mut().height = height;
        }

        let res = unsafe { av_frame_get_buffer(frame.as_ptr(), 0) };
        if res < 0 {
            return Err(make_av_error("allocating frame buffer", res));
        }

        Ok(Self { frame })
    }

    pub fn width(&self) -> i32 {
        unsafe { self.frame.as_ref().width }
    }

    pub fn height(&self) -> i32 {
        unsafe { self.frame.as_ref().height }
    }

    pub fn pixel_format(&self) -> i32 {
        unsafe { self.frame.as_ref().format }
    }

    pub fn ensure_writeable(&mut self) -> Result<(), Box<dyn Error>> {
        let result = unsafe { av_frame_make_writable(self.frame.as_ptr()) };
        if result < 0 {
            Err(make_av_error("making frame writeable", result))
        } else {
            Ok(())
        }
    }

    pub fn set_pts(&mut self, pts: i64) {
        unsafe {
            self.frame.as_mut().pts = pts;
        }
    }

    #[cfg(feature = "cairo")]
    pub fn fill_from_cairo_rgb(
        &mut self,
        cairo_surface: &cairo::ImageSurface,
    ) -> Result<(), Box<dyn Error>> {
        self.ensure_writeable()?;

        let width = self.width() as usize;
        let height = self.height() as usize;

        if cairo_surface.width() as usize != width || cairo_surface.height() as usize != height {
            return Err("Cairo surface does not match frame size!".into());
        }

        if cairo_surface.format() != cairo::Format::Rgb24
            && cairo_surface.format() != cairo::Format::ARgb32
        {
            return Err("Only CAIRO_FORMAT_RGB24 and CAIRO_FORMAT_ARGB32 are supported".into());
        }

        let frame_stride = self.linesize()[0] as usize;
        let cairo_stride = cairo_surface.stride() as usize;
        // TODO: Is it possible for sws_scale to work with the cairo data directly?
        // That could avoid this copy.
        cairo_surface.with_data(|cairo_data| {
            for y in 0..height {
                let line_data = &cairo_data[y * cairo_stride..];
                let base_offset = y * frame_stride;
                for x in 0..width {
                    // each pixel is a 32-bit quantity, with the upper 8 bits unused.
                    // Red, Green, and Blue are stored in the remaining 24 bits in that order.
                    // https://www.cairographics.org/manual-1.2.0/cairo-Image-Surfaces.html
                    let (r, g, b) = if cfg!(target_endian = "big") {
                        (
                            // line_data[x * 4 + 0], // alpha
                            line_data[x * 4 + 1],
                            line_data[x * 4 + 2],
                            line_data[x * 4 + 3],
                        )
                    } else {
                        (
                            // line_data[x * 4 + 3], // alpha
                            line_data[x * 4 + 2],
                            line_data[x * 4 + 1],
                            line_data[x * 4],
                        )
                    };

                    let base_offset = base_offset + (3 * x);

                    unsafe {
                        *self.frame.as_mut().data[0].add(base_offset) = r;
                        *self.frame.as_mut().data[0].add(base_offset + 1) = g;
                        *self.frame.as_mut().data[0].add(base_offset + 2) = b;
                    }
                }
            }
        })?;

        Ok(())
    }

    // These functions are unsafe because they return references to internal data
    // as raw pointers; it is the caller's responsibility to ensure that they don't
    // outlive self.

    pub fn data(&self) -> &[*const u8] {
        unsafe {
            std::slice::from_raw_parts(
                self.frame.as_ref().data.as_ptr() as *const *const u8,
                self.frame.as_ref().data.len(),
            )
        }
    }

    pub fn data_mut(&mut self) -> &[*mut u8] {
        unsafe { self.frame.as_mut().data.as_slice() }
    }

    pub fn linesize(&self) -> &[i32] {
        unsafe { self.frame.as_ref().linesize.as_slice() }
    }

    pub unsafe fn as_raw(&self) -> *const AVFrame {
        self.frame.as_ptr()
    }
}
impl Drop for Frame {
    fn drop(&mut self) {
        let frame_ptr = std::mem::replace(&mut self.frame, NonNull::dangling());
        let mut raw_frame_ptr = frame_ptr.as_ptr();
        unsafe { av_frame_free(&mut raw_frame_ptr) };
    }
}
