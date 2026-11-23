use hbb_common::{anyhow::Error, bail, log, ResultType};
use ndk::media::media_codec::{MediaCodec, MediaCodecDirection, MediaFormat};
use std::ops::Deref;
use std::{
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use crate::ImageFormat;
use crate::{
    codec::{EncoderApi, EncoderCfg},
    CodecFormat, I420ToABGR, I420ToARGB, ImageRgb,
};

/// MediaCodec mime type name
const H264_MIME_TYPE: &str = "video/avc";
const H265_MIME_TYPE: &str = "video/hevc";
// const VP8_MIME_TYPE: &str = "video/x-vnd.on2.vp8";
// const VP9_MIME_TYPE: &str = "video/x-vnd.on2.vp9";

// TODO MediaCodecEncoder

pub static H264_DECODER_SUPPORT: AtomicBool = AtomicBool::new(false);
pub static H265_DECODER_SUPPORT: AtomicBool = AtomicBool::new(false);

pub struct MediaCodecDecoder {
    decoder: MediaCodec,
    name: String,
}

impl Deref for MediaCodecDecoder {
    type Target = MediaCodec;

    fn deref(&self) -> &Self::Target {
        &self.decoder
    }
}

impl MediaCodecDecoder {
    pub fn new(format: CodecFormat) -> Option<MediaCodecDecoder> {
        match format {
            CodecFormat::H264 => create_media_codec(H264_MIME_TYPE, MediaCodecDirection::Decoder),
            CodecFormat::H265 => create_media_codec(H265_MIME_TYPE, MediaCodecDirection::Decoder),
            _ => {
                log::error!("Unsupported codec format: {}", format);
                None
            }
        }
    }

    // rgb [in/out] fmt and stride must be set in ImageRgb
    pub fn decode(&mut self, data: &[u8], rgb: &mut ImageRgb) -> ResultType<bool> {
        // take dst_stride into account please
        let dst_stride = rgb.stride();
        match self.dequeue_input_buffer(Duration::from_millis(10))? {
            Some(mut input_buffer) => {
                let mut buf = input_buffer.buffer_mut();
                if data.len() > buf.len() {
                    log::error!("Failed to decode, the input data size is bigger than input buf");
                    bail!("The input data size is bigger than input buf");
                }
                buf.write_all(&data)?;
                self.queue_input_buffer(input_buffer, 0, data.len(), 0, 0)?;
            }
            None => {
                log::debug!("Failed to dequeue_input_buffer: No available input_buffer");
            }
        };

        return match self.dequeue_output_buffer(Duration::from_millis(100))? {
            Some(output_buffer) => {
                let res_format = self.output_format();
                let w = res_format
                    .i32("width")
                    .ok_or(Error::msg("Failed to dequeue_output_buffer, width is None"))?
                    as usize;
                let h = res_format.i32("height").ok_or(Error::msg(
                    "Failed to dequeue_output_buffer, height is None",
                ))? as usize;
                // NOTE: 下面按 I420（COLOR_FormatYUV420Planar = 19）解析输出缓冲。
                // 部分设备即使请求了 19 也会返回 NV12(21)/Flexible，那样画面会绿屏/花屏。
                // 这里打一行日志，便于直接从 rs_rCURRENT.log 判断，不必再白编译一轮。
                if let Some(cf) = res_format.i32("color-format") {
                    if cf != 19 {
                        log::warn!(
                            "mediacodec output color-format={} (expected 19 planar), image may be broken",
                            cf
                        );
                    }
                }
                let stride = res_format.i32("stride").ok_or(Error::msg(
                    "Failed to dequeue_output_buffer, stride is None",
                ))?;
                let buf = output_buffer.buffer();
                let bps = 4;
                let u = buf.len() * 2 / 3;
                let v = buf.len() * 5 / 6;
                rgb.raw.resize(h * w * bps, 0);
                let y_ptr = buf.as_ptr();
                let u_ptr = buf[u..].as_ptr();
                let v_ptr = buf[v..].as_ptr();
                unsafe {
                    match rgb.fmt() {
                        ImageFormat::ARGB => {
                            I420ToARGB(
                                y_ptr,
                                stride,
                                u_ptr,
                                stride / 2,
                                v_ptr,
                                stride / 2,
                                rgb.raw.as_mut_ptr(),
                                (w * bps) as _,
                                w as _,
                                h as _,
                            );
                        }
                        ImageFormat::ARGB => {
                            I420ToABGR(
                                y_ptr,
                                stride,
                                u_ptr,
                                stride / 2,
                                v_ptr,
                                stride / 2,
                                rgb.raw.as_mut_ptr(),
                                (w * bps) as _,
                                w as _,
                                h as _,
                            );
                        }
                        _ => {
                            bail!("Unsupported image format");
                        }
                    }
                }
                self.release_output_buffer(output_buffer, false)?;
                Ok(true)
            }
            None => {
                log::debug!("Failed to dequeue_output: No available dequeue_output");
                Ok(false)
            }
        };
    }
}

fn create_media_codec(name: &str, direction: MediaCodecDirection) -> Option<MediaCodecDecoder> {
    let codec = MediaCodec::from_decoder_type(name)?;
    let media_format = MediaFormat::new();
    media_format.set_str("mime", name);
    media_format.set_i32("width", 0);
    media_format.set_i32("height", 0);
    media_format.set_i32("color-format", 19); // COLOR_FormatYUV420Planar
    if let Err(e) = codec.configure(&media_format, None, direction) {
        log::error!("Failed to init decoder: {:?}", e);
        return None;
    };
    log::info!("decoder init success: {:?}", name);
    if let Err(e) = codec.start() {
        log::error!("Failed to start decoder: {:?}", e);
        return None;
    };
    log::debug!("Init decoder succeeded!: {:?}", name);
    return Some(MediaCodecDecoder {
        decoder: codec,
        name: name.to_owned(),
    });
}

pub fn check_mediacodec() {
    std::thread::spawn(move || {
        // FIXME: 上游这里调用 `MediaCodecDecoder::new_decoders()`，但该方法在 1.5.3 源码里
        // 根本没有定义（半成品），一旦启用 mediacodec feature 就直接编译失败。
        // 这里改成"按 MIME 查解码器是否存在"来探测，不走 configure：
        // 探测的语义是"这台机器有没有该格式的硬解器"，而 configure 的成败依赖宽高/码流参数，
        // 属于运行期问题，不该让它把能力上报打成 false（那会让对端放弃 H264/H265）。
        // 查的是 AMediaCodec_createDecoderByType，不依赖 Kotlin 侧的 codec 列表
        // （setCodecInfo 目前会把所有 decoder 丢弃）。
        H264_DECODER_SUPPORT.store(probe_decoder(CodecFormat::H264), Ordering::SeqCst);
        H265_DECODER_SUPPORT.store(probe_decoder(CodecFormat::H265), Ordering::SeqCst);
        log::info!(
            "mediacodec support: h264={} h265={}",
            H264_DECODER_SUPPORT.load(Ordering::SeqCst),
            H265_DECODER_SUPPORT.load(Ordering::SeqCst)
        );
    });
}

fn probe_decoder(format: CodecFormat) -> bool {
    let mime = match format {
        CodecFormat::H264 => H264_MIME_TYPE,
        CodecFormat::H265 => H265_MIME_TYPE,
        _ => return false,
    };
    // 只查"解码器存不存在"，不做 configure / start。
    // MediaCodec 的 Drop 会 release 原生句柄，不会泄漏实例。
    let exists = MediaCodec::from_decoder_type(mime).is_some();
    log::info!("mediacodec probe {} -> {}", mime, exists);
    exists
}
