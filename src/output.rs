use std::{
    error::Error,
    ffi::{CStr, CString},
    ops::{Deref, DerefMut},
    path::Path,
    ptr,
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

use crate::{frame::Frame, make_av_error, X264Preset};

pub(crate) struct OutputFormatContext {
    filename: CString,
    context: *mut AVFormatContext,
}
impl OutputFormatContext {
    pub fn new<P: AsRef<Path>>(filename: P) -> Result<Self, Box<dyn Error>> {
        let mut context = ptr::null_mut();

        let filename = CString::new(filename.as_ref().to_str().unwrap().as_bytes()).unwrap();

        let result = unsafe {
            avformat_alloc_output_context2(
                &mut context,
                ptr::null_mut(),
                ptr::null_mut(),
                filename.as_bytes_with_nul().as_ptr() as *mut i8,
            )
        };

        if context.is_null() {
            if result < 0 {
                Err(make_av_error("allocating file format context", result))
            } else {
                Err(
                    "Unspecified error: could not determine output format from file extension"
                        .into(),
                )
            }
        } else {
            Ok(Self { filename, context })
        }
    }

    pub fn open_file(&mut self) -> Result<(), Box<dyn Error>> {
        let result = unsafe {
            avio_open(
                &mut (*self.context).pb,
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
        let result = unsafe { avformat_write_header(self.context, &mut opts) };

        unsafe { av_dict_free(&mut opts) };

        if result < 0 {
            Err(make_av_error("writing header to output file", result))
        } else {
            Ok(())
        }
    }

    // Must call open_file before this
    pub fn write_trailer(&mut self) -> Result<(), Box<dyn Error>> {
        let result = unsafe { av_write_trailer(self.context) };

        if result < 0 {
            Err(make_av_error("writing trailer to output file", result))
        } else {
            Ok(())
        }
    }

    pub fn add_stream<'a>(
        &'a mut self,
        codec_id: AVCodecID,
        width: i32,
        height: i32,
        framerate: i32,
        pixel_format: AVPixelFormat,
        bit_rate: Option<i64>,
        gop_size: Option<i32>,
    ) -> Result<(OutputStream, *mut AVCodec), Box<dyn Error>> {
        let codec = unsafe { avcodec_find_encoder(codec_id) };
        if codec.is_null() {
            let name = unsafe { avcodec_get_name(codec_id) };
            let error_action = format!(
                "Error finding encoder for codec {}",
                unsafe { CStr::from_ptr(name) }
                    .to_str()
                    .expect("avcodec_get_name returned invalid UTF-8")
            );
            return Err(error_action.into());
        }

        if unsafe { (*codec).type_ } != AVMediaType::AVMEDIA_TYPE_VIDEO {
            return Err("Error: the specified codec is not a video codec".into());
        }

        Ok((
            OutputStream::new(
                self,
                width,
                height,
                framerate,
                codec,
                codec_id,
                pixel_format,
                bit_rate,
                gop_size,
            )?,
            codec,
        ))
    }
}
impl Drop for OutputFormatContext {
    fn drop(&mut self) {
        unsafe {
            avio_closep(&mut (*self.context).pb);
            avformat_free_context(self.context);
        }
    }
}

struct AVCodecContextWrapper {
    codec_context: *mut AVCodecContext,
}
impl AVCodecContextWrapper {
    fn new(codec: *mut AVCodec) -> Result<Self, Box<dyn Error>> {
        let codec_context = unsafe { avcodec_alloc_context3(codec) };
        if codec_context.is_null() {
            Err("Error allocating AVCodecContext".into())
        } else {
            Ok(Self { codec_context })
        }
    }

    fn finish(&self) -> Result<(), Box<dyn Error>> {
        let result = unsafe { avcodec_send_frame(self.codec_context, ptr::null_mut()) };
        if result < 0 {
            Err(make_av_error("sending EOF to encoder", result))
        } else {
            Ok(())
        }
    }

    fn send_frame(&self, frame: &Frame) -> Result<(), Box<dyn Error>> {
        let result = unsafe { avcodec_send_frame(self.codec_context, frame.as_raw()) };
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
        stream: *mut AVStream,
    ) -> Result<(), Box<dyn Error>> {
        let mut res = 0;
        while res >= 0 {
            res = unsafe { avcodec_receive_packet(self.codec_context, packet.packet) };
            if res == AVERROR(EAGAIN) || res == AVERROR_EOF {
                break;
            } else if res < 0 {
                return Err(make_av_error("encoding a frame", res));
            }

            unsafe {
                av_packet_rescale_ts(
                    packet.packet,
                    (*self.codec_context).time_base,
                    (*stream).time_base,
                );
                (*packet.packet).stream_index = (*stream).index;
            }

            res = unsafe { av_interleaved_write_frame(output_context.context, packet.packet) };
            if res < 0 {
                return Err(make_av_error("writing output packet", res));
            }
        }

        Ok(())
    }
}
impl Drop for AVCodecContextWrapper {
    fn drop(&mut self) {
        unsafe { avcodec_free_context(&mut self.codec_context) };
    }
}
impl Deref for AVCodecContextWrapper {
    type Target = AVCodecContext;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.codec_context }
    }
}
impl DerefMut for AVCodecContextWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.codec_context }
    }
}

struct AVPacketWrapper {
    packet: *mut AVPacket,
}
impl AVPacketWrapper {
    fn new() -> Result<Self, Box<dyn Error>> {
        let packet = unsafe { av_packet_alloc() };

        if packet.is_null() {
            Err("Error allocating AVPacket".into())
        } else {
            Ok(Self { packet })
        }
    }
}
impl Drop for AVPacketWrapper {
    fn drop(&mut self) {
        unsafe { av_packet_free(&mut self.packet) };
    }
}

struct SwsContextWrapper {
    sws_ctx: *mut SwsContext,
}
impl SwsContextWrapper {
    fn new(src: &Frame, dest: &Frame) -> Result<Self, Box<dyn Error>> {
        let sws_ctx = unsafe {
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
        };

        if sws_ctx.is_null() {
            Err("Error initializing SwsContext".into())
        } else {
            Ok(Self { sws_ctx })
        }
    }

    fn scale(&self, src: &Frame, dest: &mut Frame, height: i32) -> Result<(), Box<dyn Error>> {
        dest.ensure_writeable()?;

        unsafe {
            sws_scale(
                self.sws_ctx,
                src.data(),
                src.linesize(),
                0,
                height, // TODO: can this be src.height() ?
                dest.data_mut(),
                dest.linesize(),
            );
        }

        Ok(())
    }
}
impl Drop for SwsContextWrapper {
    fn drop(&mut self) {
        unsafe { sws_freeContext(self.sws_ctx) }
    }
}

pub(crate) struct OutputStream {
    stream: *mut AVStream,
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
        codec: *mut AVCodec,
        codec_id: AVCodecID,
        pixel_format: AVPixelFormat,
        bit_rate: Option<i64>,
        gop_size: Option<i32>,
    ) -> Result<Self, Box<dyn Error>> {
        let stream = unsafe { avformat_new_stream(format_context.context, ptr::null_mut()) };
        if stream.is_null() {
            return Err("Error allocating AVStream".into());
        }
        unsafe {
            (*stream).id = ((*format_context.context).nb_streams - 1) as i32;
        }

        let mut encoder_context = AVCodecContextWrapper::new(codec)?;
        encoder_context.codec_id = codec_id;
        encoder_context.bit_rate = bit_rate.unwrap_or(800_000);
        encoder_context.width = width;
        encoder_context.height = height;
        unsafe {
            (*stream).time_base.num = 1;
            (*stream).time_base.den = framerate;
        }
        encoder_context.time_base = unsafe { (*stream).time_base };
        encoder_context.gop_size = gop_size.unwrap_or(10);
        encoder_context.pix_fmt = pixel_format;

        if unsafe { (*format_context.context).flags } & AVFMT_GLOBALHEADER != 0 {
            encoder_context.flags |= AV_CODEC_FLAG_GLOBAL_HEADER as i32;
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
        codec: *const AVCodec,
        preset: Option<X264Preset>,
        crf: Option<i64>,
    ) -> Result<(), Box<dyn Error>> {
        let mut options = ptr::null_mut();

        let preset = preset.unwrap_or(X264Preset::Medium).as_bytes_with_nul();
        unsafe {
            av_dict_set(&mut options, "preset\0".as_ptr() as *const i8, preset, 0);
        }

        if let Some(crf) = crf {
            unsafe {
                av_dict_set_int(&mut options, "crf\0".as_ptr() as *const i8, crf, 0);
            }
        }

        let result =
            unsafe { avcodec_open2(self.encoder_context.codec_context, codec, &mut options) };
        unsafe { av_dict_free(&mut options) };
        if result < 0 {
            return Err(make_av_error("opening video codec", result));
        }

        let result = unsafe {
            avcodec_parameters_from_context(
                (*self.stream).codecpar,
                self.encoder_context.codec_context,
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
        let frame_to_send =
            if self.encoder_context.pix_fmt as i32 != frame.pixel_format() {
                if self.sws_context.is_none() {
                    self.sws_context = Some(SwsContextWrapper::new(&frame, &self.temp_frame)?);
                }
                self.sws_context.as_ref().unwrap().scale(
                    frame,
                    &mut self.temp_frame,
                    self.encoder_context.height,
                )?;

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
