use std::{
    error::Error,
    ffi::{CStr, CString},
    path::Path,
    ptr::{self, NonNull},
};

use ffmpeg_sys_next::{
    av_dict_free, av_dict_set, av_dict_set_int, av_interleaved_write_frame, av_packet_alloc,
    av_packet_free, av_packet_rescale_ts, av_write_trailer, avcodec_alloc_context3,
    avcodec_find_encoder, avcodec_free_context, avcodec_get_name, avcodec_open2,
    avcodec_parameters_from_context, avcodec_receive_packet, avcodec_send_frame,
    avformat_alloc_output_context2, avformat_free_context, avformat_new_stream,
    avformat_write_header, avio_closep, avio_open, sws_freeContext, sws_getContext, sws_scale,
    AVCodec, AVCodecContext, AVCodecID, AVFormatContext, AVMediaType, AVPacket, AVPixelFormat,
    AVStream, SwsContext, AVERROR, AVERROR_EOF, AVFMT_GLOBALHEADER, AVIO_FLAG_WRITE,
    AV_CODEC_FLAG_GLOBAL_HEADER, EAGAIN, SWS_BICUBIC,
};

use crate::{frame::Frame, make_av_error, OptionalSettings, X264Preset};

pub(crate) struct OutputFormatContext {
    filename: CString,
    context: NonNull<AVFormatContext>,
}
impl OutputFormatContext {
    pub fn new<P: AsRef<Path>>(filename: P) -> Result<Self, Box<dyn Error>> {
        let mut context = ptr::null_mut();

        let filename = CString::new(
            filename
                .as_ref()
                .to_str()
                .ok_or("Filename is invalid UTF-8")?
                .as_bytes(),
        )?;

        let result = unsafe {
            avformat_alloc_output_context2(
                &mut context,
                ptr::null_mut(),
                ptr::null_mut(),
                filename.as_bytes_with_nul().as_ptr() as *mut i8,
            )
        };

        let Some(context) = NonNull::new(context) else {
            if result < 0 {
                return Err(make_av_error("allocating file format context", result))
            } else {
                return Err(
                    "Unspecified error: could not determine output format from file extension"
                        .into(),
                )
            }
        };

        Ok(Self { filename, context })
    }

    pub fn open_file(&mut self) -> Result<(), Box<dyn Error>> {
        let result = unsafe {
            avio_open(
                &mut self.context.as_mut().pb,
                self.filename.as_bytes_with_nul().as_ptr() as *mut i8,
                AVIO_FLAG_WRITE,
            )
        };

        if result < 0 {
            Err(make_av_error("opening destination file", result))
        } else {
            Ok(())
        }
    }

    // Must call open_file and add_stream before this
    pub fn write_header(&mut self) -> Result<(), Box<dyn Error>> {
        let mut opts = ptr::null_mut();

        // Safety: the lifetime of the data behind self.context is the same as the
        // lifetime of self, and it is guaranteed to be non-null by the constructor.
        let result = unsafe { avformat_write_header(self.context.as_ptr(), &mut opts) };

        unsafe { av_dict_free(&mut opts) };

        if result < 0 {
            Err(make_av_error("writing header to output file", result))
        } else {
            Ok(())
        }
    }

    // Must call open_file before this
    pub fn write_trailer(&mut self) -> Result<(), Box<dyn Error>> {
        let result = unsafe { av_write_trailer(self.context.as_ptr()) };

        if result < 0 {
            Err(make_av_error("writing trailer to output file", result))
        } else {
            Ok(())
        }
    }

    pub fn add_stream(
        &mut self,
        codec_id: AVCodecID,
        width: i32,
        height: i32,
        framerate: i32,
        pixel_format: AVPixelFormat,
        settings: &OptionalSettings,
    ) -> Result<(OutputStream, NonNull<AVCodec>), Box<dyn Error>> {
        let Some(codec) = NonNull::new(unsafe { avcodec_find_encoder(codec_id) }) else {
            let name = unsafe { avcodec_get_name(codec_id) };
            let error_action = format!(
                "Error finding encoder for codec {}",
                unsafe { CStr::from_ptr(name) }
                    .to_str()
                    .expect("avcodec_get_name returned invalid UTF-8")
            );
            return Err(error_action.into());
        };

        if unsafe { codec.as_ref().type_ } != AVMediaType::AVMEDIA_TYPE_VIDEO {
            return Err("Error: the specified codec is not a video codec".into());
        }

        Ok((
            OutputStream::new(
                self,
                width,
                height,
                framerate,
                codec,
                pixel_format,
                settings,
            )?,
            codec,
        ))
    }
}
impl Drop for OutputFormatContext {
    fn drop(&mut self) {
        unsafe {
            avio_closep(&mut self.context.as_mut().pb);
            avformat_free_context(self.context.as_ptr());
        }
    }
}

struct AVCodecContextWrapper {
    codec_context: NonNull<AVCodecContext>,
}
impl AVCodecContextWrapper {
    fn new(codec: NonNull<AVCodec>) -> Result<Self, Box<dyn Error>> {
        let Some(codec_context) = NonNull::new(unsafe { avcodec_alloc_context3(codec.as_ptr()) }) else {
            return Err("Error allocating AVCodecContext".into());
        };
        Ok(Self { codec_context })
    }

    fn finish(&self) -> Result<(), Box<dyn Error>> {
        let result = unsafe { avcodec_send_frame(self.codec_context.as_ptr(), ptr::null_mut()) };
        if result < 0 {
            Err(make_av_error("sending EOF to encoder", result))
        } else {
            Ok(())
        }
    }

    fn send_frame(&self, frame: &Frame) -> Result<(), Box<dyn Error>> {
        let result = unsafe { avcodec_send_frame(self.codec_context.as_ptr(), frame.as_raw()) };
        if result < 0 {
            Err(make_av_error("sending frame to encoder", result))
        } else {
            Ok(())
        }
    }

    fn flush(
        &self,
        output_context: &OutputFormatContext,
        packet: &mut AVPacketWrapper,
        stream: NonNull<AVStream>,
    ) -> Result<(), Box<dyn Error>> {
        let mut res = 0;
        while res >= 0 {
            res = unsafe {
                avcodec_receive_packet(self.codec_context.as_ptr(), packet.packet.as_ptr())
            };
            if res == AVERROR(EAGAIN) || res == AVERROR_EOF {
                break;
            } else if res < 0 {
                return Err(make_av_error("encoding a frame", res));
            }

            unsafe {
                av_packet_rescale_ts(
                    packet.packet.as_ptr(),
                    self.codec_context.as_ref().time_base,
                    stream.as_ref().time_base,
                );
                packet.packet.as_mut().stream_index = stream.as_ref().index;
            }

            res = unsafe {
                av_interleaved_write_frame(output_context.context.as_ptr(), packet.packet.as_ptr())
            };
            if res < 0 {
                return Err(make_av_error("writing output packet", res));
            }
        }

        Ok(())
    }
}
impl Drop for AVCodecContextWrapper {
    fn drop(&mut self) {
        let context_ptr = std::mem::replace(&mut self.codec_context, NonNull::dangling());
        let mut raw_context_ptr = context_ptr.as_ptr();
        unsafe { avcodec_free_context(&mut raw_context_ptr) };
    }
}

struct AVPacketWrapper {
    packet: NonNull<AVPacket>,
}
impl AVPacketWrapper {
    fn new() -> Result<Self, Box<dyn Error>> {
        let Some(packet) = NonNull::new(unsafe { av_packet_alloc() }) else {
            return Err("Error allocating AVPacket".into());
        };
        Ok(Self { packet })
    }
}
impl Drop for AVPacketWrapper {
    fn drop(&mut self) {
        let packet_ptr = std::mem::replace(&mut self.packet, NonNull::dangling());
        let mut raw_frame_ptr = packet_ptr.as_ptr();
        unsafe { av_packet_free(&mut raw_frame_ptr) };
    }
}

struct SwsContextWrapper {
    sws_ctx: NonNull<SwsContext>,
}
impl SwsContextWrapper {
    fn new(src: &Frame, dest: &Frame) -> Result<Self, Box<dyn Error>> {
        let Some(sws_ctx) = NonNull::new(unsafe {
            sws_getContext(
                src.width(),
                src.height(),
                std::mem::transmute_copy(&src.pixel_format()),
                dest.width(),
                dest.height(),
                std::mem::transmute_copy(&dest.pixel_format()),
                SWS_BICUBIC,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        }) else {
            return Err("Error initializing SwsContext".into());
        };

        Ok(Self { sws_ctx })
    }

    fn scale(&self, src: &Frame, dest: &mut Frame, height: i32) -> Result<(), Box<dyn Error>> {
        dest.ensure_writeable()?;

        unsafe {
            sws_scale(
                self.sws_ctx.as_ptr(),
                src.data().as_ptr(),
                src.linesize().as_ptr(),
                0,
                height, // TODO: can this be src.height() ?
                dest.data_mut().as_ptr(),
                dest.linesize().as_ptr(),
            );
        }

        Ok(())
    }
}
impl Drop for SwsContextWrapper {
    fn drop(&mut self) {
        unsafe { sws_freeContext(self.sws_ctx.as_ptr()) }
    }
}

pub(crate) struct OutputStream {
    stream: NonNull<AVStream>,
    encoder_context: AVCodecContextWrapper,

    next_pts: i64,

    // used as temporary destination buffer for conversion when input frame has wrong pixel format
    temp_frame: Frame,
    sws_context: Option<SwsContextWrapper>,

    packet: AVPacketWrapper,
}
impl OutputStream {
    fn new(
        format_context: &OutputFormatContext,
        width: i32,
        height: i32,
        framerate: i32,
        codec: NonNull<AVCodec>,
        pixel_format: AVPixelFormat,
        settings: &OptionalSettings,
    ) -> Result<Self, Box<dyn Error>> {
        let Some(mut stream) = NonNull::new(unsafe { avformat_new_stream(format_context.context.as_ptr(), ptr::null_mut()) }) else {
            return Err("Error allocating AVStream".into());
        };
        unsafe {
            stream.as_mut().id = (format_context.context.as_ref().nb_streams - 1) as i32;
        }

        let mut encoder_context = AVCodecContextWrapper::new(codec)?;

        unsafe {
            encoder_context.codec_context.as_mut().codec_id = codec.as_ref().id;
            encoder_context.codec_context.as_mut().bit_rate = settings.bitrate.unwrap_or(800_000);
            encoder_context.codec_context.as_mut().width = width;
            encoder_context.codec_context.as_mut().height = height;
            stream.as_mut().time_base.num = 1;
            stream.as_mut().time_base.den = framerate;
            encoder_context.codec_context.as_mut().time_base = stream.as_ref().time_base;
            encoder_context.codec_context.as_mut().gop_size = settings.gop_size.unwrap_or(10);
            encoder_context.codec_context.as_mut().pix_fmt = pixel_format;

            if format_context.context.as_ref().flags & AVFMT_GLOBALHEADER != 0 {
                encoder_context.codec_context.as_mut().flags |= AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
        }

        Ok(Self {
            stream,
            encoder_context,
            next_pts: 0,
            temp_frame: Frame::new(pixel_format, width, height)?,
            packet: AVPacketWrapper::new()?,
            sws_context: None,
        })
    }

    pub fn open_video(
        &mut self,
        codec: NonNull<AVCodec>,
        settings: &OptionalSettings,
    ) -> Result<(), Box<dyn Error>> {
        let mut options = ptr::null_mut();

        let preset = settings
            .preset
            .unwrap_or(X264Preset::Medium)
            .as_bytes_with_nul();
        unsafe {
            av_dict_set(&mut options, "preset\0".as_ptr() as *const i8, preset, 0);
        }

        if let Some(crf) = settings.crf {
            unsafe {
                av_dict_set_int(&mut options, "crf\0".as_ptr() as *const i8, crf, 0);
            }
        }

        let result = unsafe {
            avcodec_open2(
                self.encoder_context.codec_context.as_ptr(),
                codec.as_ptr(),
                &mut options,
            )
        };
        unsafe { av_dict_free(&mut options) };
        if result < 0 {
            return Err(make_av_error("opening video codec", result));
        }

        let result = unsafe {
            avcodec_parameters_from_context(
                self.stream.as_ref().codecpar,
                self.encoder_context.codec_context.as_ptr(),
            )
        };
        if result < 0 {
            return Err(make_av_error("copying stream parameters", result));
        }

        Ok(())
    }

    pub fn write_frame(
        &mut self,
        frame: &mut Frame,
        output_context: &OutputFormatContext,
    ) -> Result<(), Box<dyn Error>> {
        let frame_to_send = if unsafe { self.encoder_context.codec_context.as_ref().pix_fmt as i32 }
            != frame.pixel_format()
        {
            if self.sws_context.is_none() {
                self.sws_context = Some(SwsContextWrapper::new(frame, &self.temp_frame)?);
            }
            self.sws_context
                .as_ref()
                .unwrap()
                .scale(frame, &mut self.temp_frame, unsafe {
                    self.encoder_context.codec_context.as_ref().height
                })?;

            &mut self.temp_frame
        } else {
            frame
        };

        frame_to_send.set_pts(self.next_pts);
        self.next_pts += 1;

        self.encoder_context.send_frame(frame_to_send)?;

        self.encoder_context
            .flush(output_context, &mut self.packet, self.stream)?;
        Ok(())
    }

    pub fn finish(&mut self, output_context: &OutputFormatContext) -> Result<(), Box<dyn Error>> {
        self.encoder_context.finish()?;
        self.encoder_context
            .flush(output_context, &mut self.packet, self.stream)?;
        Ok(())
    }
}
