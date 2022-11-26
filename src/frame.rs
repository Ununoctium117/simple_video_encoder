use std::error::Error;

use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_free, av_frame_get_buffer, av_frame_make_writable, AVFrame,
    AVPixelFormat,
};

use crate::make_av_error;

pub(crate) struct Frame {
    frame: *mut AVFrame,
}
impl Frame {
    pub fn new(fmt: AVPixelFormat, width: i32, height: i32) -> Result<Self, Box<dyn Error>> {
        let frame = unsafe { av_frame_alloc() };
        if frame.is_null() {
            return Err("Error allocating AVFrame".into());
        }

        unsafe {
            (*frame).format = fmt as i32;
            (*frame).width = width;
            (*frame).height = height;
        }

        let res = unsafe { av_frame_get_buffer(frame, 0) };
        if res < 0 {
            return Err(make_av_error("allocating frame buffer", res));
        }

        Ok(Self { frame })
    }

    pub fn width(&self) -> i32 {
        unsafe { (*self.frame).width }
    }

    pub fn height(&self) -> i32 {
        unsafe { (*self.frame).height }
    }

    pub fn pixel_format(&self) -> i32 {
        unsafe { (*self.frame).format }
    }

    pub fn ensure_writeable(&mut self) -> Result<(), Box<dyn Error>> {
        let result = unsafe { av_frame_make_writable(self.frame) };
        if result < 0 {
            Err(make_av_error("making frame writeable", result))
        } else {
            Ok(())
        }
    }

    pub fn set_pts(&mut self, pts: i64) {
        unsafe {
            (*self.frame).pts = pts;
        }
    }

    // These functions are unsafe because they return references to internal data
    // as raw pointers; it is the caller's responsibility to ensure that they don't
    // outlive self.

    pub unsafe fn data(&self) -> *const *const u8 {
        unsafe { (*self.frame).data.as_ptr() as *const *const u8 }
    }

    pub unsafe fn data_mut(&mut self) -> *const *mut u8 {
        unsafe { (*self.frame).data.as_ptr() }
    }

    pub unsafe fn linesize(&self) -> *const i32 {
        unsafe { (*self.frame).linesize.as_ptr() }
    }

    pub unsafe fn as_raw(&self) -> *const AVFrame {
        self.frame
    }

    #[cfg(feature = "cairo")]
    pub fn fill_from_cairo_rgb(
        &mut self,
        cairo_surface: &cairo::ImageSurface,
    ) -> Result<(), Box<dyn Error>> {
        self.ensure_writeable()?;

        let width = unsafe { (*self.frame).width } as usize;
        let height = unsafe { (*self.frame).height } as usize;

        if cairo_surface.width() as usize != width || cairo_surface.height() as usize != height {
            return Err("Cairo surface does not match frame size!".into());
        }

        if cairo_surface.format() != cairo::Format::Rgb24 && cairo_surface.format() != cairo::Format::ARgb32 {
            return Err("Only CAIRO_FORMAT_RGB24 and CAIRO_FORMAT_ARGB32 are supported".into());
        }

        let frame_stride = unsafe { (*self.frame).linesize[0] } as usize;
        let cairo_stride = cairo_surface.stride() as usize;
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
                            line_data[x * 4 + 0],
                        )
                    };

                    let base_offset = base_offset + (3 * x);

                    unsafe {
                        *(*self.frame).data[0].offset((base_offset + 0) as isize) = r;
                        *(*self.frame).data[0].offset((base_offset + 1) as isize) = g;
                        *(*self.frame).data[0].offset((base_offset + 2) as isize) = b;
                    }
                }
            }
        })?;

        Ok(())
    }
}
impl Drop for Frame {
    fn drop(&mut self) {
        unsafe { av_frame_free(&mut self.frame) };
    }
}
