//! Converting video with Media Foundation (Windows): any format Windows can play in, MP4 (H.264 +
//! AAC) or M4A (AAC only) out. The source reader decodes (and scales, through its video processor)
//! and the sink writer encodes, both with hardware transforms when there are any. Decision D3: 0 MB
//! in the installer; WebM/VP9 input needs the "Web Media Extensions" Windows ships with.

use super::VideoError;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{HSTRING, Interface};

#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertOptions {
    /// Only the sound, as M4A (AAC).
    pub audio_only: bool,
    /// Shrink to at most this many lines (never enlarges).
    pub max_height: Option<u32>,
    pub video_kbps: u32,
    pub audio_kbps: u32,
}

impl From<windows::core::Error> for VideoError {
    fn from(e: windows::core::Error) -> Self {
        VideoError::Unsupported { what: format!("Media Foundation: {}", e.message()) }
    }
}

/// Names the step a Media Foundation call failed in.
fn at(step: &'static str) -> impl FnOnce(windows::core::Error) -> VideoError {
    move |e| VideoError::Unsupported { what: format!("Media Foundation, {step}: {}", e.message()) }
}

const FIRST_VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const FIRST_AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const MEDIA_SOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;
/// The bit rates the Windows AAC encoder takes, in bytes per second.
const AAC_RATES: [u32; 4] = [12_000, 16_000, 20_000, 24_000];

/// Media Foundation for this thread (COM in the multithreaded apartment, then MFStartup).
fn start() -> Result<(), VideoError> {
    unsafe {
        // Already initialized on this thread (another mode) is fine for MF.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
    }
    Ok(())
}

/// Duration in seconds, and whether there are video and audio streams.
pub fn probe(src: &Path) -> Result<(f64, bool, bool), VideoError> {
    start()?;
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(src.as_os_str()), None)?;
        let duration = reader.GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION).ok().and_then(|v| u64::try_from(&v).ok()).unwrap_or(0) as f64 / 1e7;
        let video = reader.GetNativeMediaType(FIRST_VIDEO, 0).is_ok();
        let audio = reader.GetNativeMediaType(FIRST_AUDIO, 0).is_ok();
        Ok((duration, video, audio))
    }
}

/// Converts `src` into `dst` (.mp4, or .m4a with `audio_only`), calling `progress` with 0..1.
/// Written to a temporary file first; a cancel or an error leaves nothing behind.
pub fn convert(src: &Path, dst: &Path, options: &ConvertOptions, progress: &mut dyn FnMut(f64), cancel: &AtomicBool) -> Result<(), VideoError> {
    start()?;
    let ext = if options.audio_only { "m4a" } else { "mp4" };
    // The sink writer picks the container from the extension.
    let tmp = dst.with_extension(format!("jaye-tmp.{ext}"));
    let result = unsafe { run(src, &tmp, options, progress, cancel) };
    match result {
        Ok(()) => std::fs::rename(&tmp, dst).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.into()
        }),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

unsafe fn run(src: &Path, dst: &Path, options: &ConvertOptions, progress: &mut dyn FnMut(f64), cancel: &AtomicBool) -> Result<(), VideoError> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(|| VideoError::Unsupported { what: "Media Foundation attributes".into() })?;
        // The reader decodes to system memory (some hardware decoders, VP9's among them, hand out
        // GPU surfaces the encoder can't take); the writer encodes on the GPU when it can.
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(src.as_os_str()), &attrs).map_err(|e| VideoError::Unsupported { what: format!("Windows can't read this file ({})", e.message()) })?;
        let duration = reader.GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION).ok().and_then(|v: PROPVARIANT| u64::try_from(&v).ok()).unwrap_or(0).max(1) as f64;

        let has_video = !options.audio_only && reader.GetNativeMediaType(FIRST_VIDEO, 0).is_ok();
        let has_audio = reader.GetNativeMediaType(FIRST_AUDIO, 0).is_ok();
        if !has_video && !has_audio {
            return Err(VideoError::Unsupported { what: "no video or audio Windows can decode".into() });
        }
        reader.SetStreamSelection(ALL_STREAMS, false)?;
        let mut writer_attrs = None;
        MFCreateAttributes(&mut writer_attrs, 1)?;
        let writer_attrs = writer_attrs.ok_or_else(|| VideoError::Unsupported { what: "Media Foundation attributes".into() })?;
        writer_attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(dst.as_os_str()), None, &writer_attrs)?;
        // (reader stream, writer stream)
        let mut streams: Vec<(u32, u32)> = vec![];
        // Which of `streams` is the video, and the size of one decoded NV12 frame.
        let mut video_index = usize::MAX;
        let mut frame_bytes = 0u32;

        if has_video {
            reader.SetStreamSelection(FIRST_VIDEO, true)?;
            let native = reader.GetNativeMediaType(FIRST_VIDEO, 0)?;
            let size = native.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let (w, h) = ((size >> 32) as u32, size as u32);
            // WebM says 30 fps as 3 000 000 000 / 100 000 000: reduced for the encoder.
            let rate = reduce(native.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(30u64 << 32 | 1));
            let (w2, h2) = scaled(w, h, options.max_height);
            let decoded = MFCreateMediaType()?;
            decoded.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            decoded.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            decoded.SetUINT64(&MF_MT_FRAME_SIZE, (w2 as u64) << 32 | h2 as u64)?;
            decoded.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
            reader.SetCurrentMediaType(FIRST_VIDEO, None, &decoded).map_err(at("decoding the video"))?;
            let input = reader.GetCurrentMediaType(FIRST_VIDEO)?;
            let h264 = MFCreateMediaType()?;
            h264.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            h264.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            h264.SetUINT32(&MF_MT_AVG_BITRATE, options.video_kbps.clamp(100, 100_000) * 1000)?;
            h264.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            h264.SetUINT64(&MF_MT_FRAME_SIZE, (w2 as u64) << 32 | h2 as u64)?;
            h264.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
            h264.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1u64 << 32 | 1)?;
            let index = writer.AddStream(&h264).map_err(at("setting up the H.264 encoder"))?;
            writer.SetInputMediaType(index, &input, None).map_err(at("connecting the H.264 encoder"))?;
            video_index = streams.len();
            frame_bytes = w2 * h2 * 3 / 2;
            streams.push((FIRST_VIDEO, index));
        }
        if has_audio {
            reader.SetStreamSelection(FIRST_AUDIO, true)?;
            // 16-bit PCM, 48 kHz stereo, what the AAC encoder takes (the reader resamples).
            let pcm = MFCreateMediaType()?;
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            pcm.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
            pcm.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            reader.SetCurrentMediaType(FIRST_AUDIO, None, &pcm).map_err(at("decoding the audio"))?;
            let input = reader.GetCurrentMediaType(FIRST_AUDIO)?;
            let bytes_per_second = options.audio_kbps * 1000 / 8;
            let aac_rate = *AAC_RATES.iter().min_by_key(|&&r| r.abs_diff(bytes_per_second)).unwrap();
            let aac = MFCreateMediaType()?;
            aac.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            aac.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
            aac.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            aac.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
            aac.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            aac.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, aac_rate)?;
            let index = writer.AddStream(&aac).map_err(at("setting up the AAC encoder"))?;
            writer.SetInputMediaType(index, &input, None).map_err(at("connecting the AAC encoder"))?;
            streams.push((FIRST_AUDIO, index));
        }

        writer.BeginWriting().map_err(at("starting to write"))?;
        // Read from whichever stream is behind, so the file comes out interleaved.
        let mut last_ts = vec![0i64; streams.len()];
        let mut done = vec![false; streams.len()];
        let mut reported = -1.0;
        let mut start: Option<i64> = None;
        while let Some(i) = (0..streams.len()).filter(|&i| !done[i]).min_by_key(|&i| last_ts[i]) {
            if cancel.load(Ordering::Relaxed) {
                return Err(VideoError::Cancelled);
            }
            let (reader_stream, writer_stream) = streams[i];
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            reader.ReadSample(reader_stream, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).map_err(at("reading"))?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(VideoError::Invalid { message: "the file stopped decoding".into() });
            }
            // Windows' VP9 decoder fills the frame but leaves the buffer's length at 0, and the
            // encoder refuses an empty sample: a single buffer exactly one NV12 frame big gets
            // that length. Truly empty samples are skipped.
            if i == video_index
                && let Some(s) = &sample
                && s.GetTotalLength().unwrap_or(0) == 0
                && let Ok(buffer) = s.GetBufferByIndex(0)
                && buffer.GetMaxLength().unwrap_or(0) == frame_bytes
            {
                buffer.SetCurrentLength(frame_bytes).map_err(at("reading"))?;
            }
            let sample = sample.filter(|s| s.GetTotalLength().unwrap_or(0) > 0);
            if let Some(sample) = sample {
                // Times from 0 and never negative: WebM files can start below 0 or leave a
                // sample without its own time, which the sink writer refuses.
                let base = *start.get_or_insert(ts);
                sample.SetSampleTime((ts - base).max(0)).map_err(at("encoding"))?;
                writer.WriteSample(writer_stream, &sample).map_err(at("encoding"))?;
                last_ts[i] = ts;
            } else if flags & MF_SOURCE_READERF_STREAMTICK.0 as u32 != 0 {
                writer.SendStreamTick(writer_stream, (ts - start.unwrap_or(ts)).max(0))?;
                last_ts[i] = ts;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                done[i] = true;
            }
            let p = (last_ts[0] as f64 / duration).clamp(0.0, 1.0);
            if p - reported >= 0.01 {
                reported = p;
                progress(p);
            }
        }
        writer.Finalize().map_err(at("finishing the file"))?;
        progress(1.0);
        Ok(())
    }
}

/// The frame at `seconds` (the first one at or after it), as RGBA, full size: (width, height,
/// pixels, the frame's real time).
pub fn frame(src: &Path, seconds: f64) -> Result<(u32, u32, Vec<u8>, f64), VideoError> {
    start()?;
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(|| VideoError::Unsupported { what: "Media Foundation attributes".into() })?;
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(src.as_os_str()), &attrs).map_err(at("opening"))?;
        reader.SetStreamSelection(ALL_STREAMS, false)?;
        reader.SetStreamSelection(FIRST_VIDEO, true).map_err(at("finding the video"))?;
        let native = reader.GetNativeMediaType(FIRST_VIDEO, 0)?;
        let size = native.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (w, h) = ((size >> 32) as u32, size as u32);
        if w == 0 || h == 0 || w as u64 * h as u64 > 300_000_000 {
            return Err(VideoError::TooBig);
        }
        let rgb = MFCreateMediaType()?;
        rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        rgb.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        reader.SetCurrentMediaType(FIRST_VIDEO, None, &rgb).map_err(at("decoding the video"))?;
        let target = (seconds.max(0.0) * 1e7) as i64;
        reader.SetCurrentPosition(&windows::core::GUID::zeroed(), &PROPVARIANT::from(target)).map_err(at("seeking"))?;
        // Seeking lands on the key frame before: decode forward to the frame asked for.
        let mut last = None;
        for _ in 0..100_000 {
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).map_err(at("reading"))?;
            if let Some(sample) = sample {
                let done = ts + sample.GetSampleDuration().unwrap_or(0) > target;
                last = Some((sample, ts));
                if done {
                    break;
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        let (sample, ts) = last.ok_or_else(|| VideoError::Invalid { message: "no frame there".into() })?;
        let buffer = sample.ConvertToContiguousBuffer().map_err(at("reading the frame"))?;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        // A 2D buffer knows its pitch (and whether rows go bottom-up); else rows are w * 4.
        if let Ok(b2d) = buffer.cast::<IMF2DBuffer>() {
            let (mut line0, mut pitch) = (std::ptr::null_mut(), 0i32);
            b2d.Lock2D(&mut line0, &mut pitch).map_err(at("reading the frame"))?;
            for y in 0..h as isize {
                let row = std::slice::from_raw_parts(line0.offset(y * pitch as isize), (w * 4) as usize);
                rgba[(y as usize * w as usize * 4)..][..(w * 4) as usize].copy_from_slice(row);
            }
            b2d.Unlock2D()?;
        } else {
            let (mut data, mut max, mut len) = (std::ptr::null_mut(), 0u32, 0u32);
            buffer.Lock(&mut data, Some(&mut max), Some(&mut len))?;
            let n = (len as usize).min(rgba.len());
            rgba[..n].copy_from_slice(std::slice::from_raw_parts(data, n));
            buffer.Unlock()?;
        }
        // RGB32 is BGRX in memory: to RGBA, opaque.
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        Ok((w, h, rgba, ts as f64 / 1e7))
    }
}

/// A frame rate `num << 32 | den` with the fraction reduced (and 30 fps if it's nonsense).
fn reduce(rate: u64) -> u64 {
    let (mut num, mut den) = (rate >> 32, rate & 0xffff_ffff);
    if num == 0 || den == 0 {
        return 30 << 32 | 1;
    }
    let (mut a, mut b) = (num, den);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    num /= a;
    den /= a;
    num << 32 | den
}

/// The size after shrinking to `max_height` lines, both even (H.264 wants even sizes).
fn scaled(w: u32, h: u32, max_height: Option<u32>) -> (u32, u32) {
    let even = |v: u32| (v.max(2)) & !1;
    match max_height {
        Some(m) if m > 0 && m < h => (even((w as u64 * m as u64 / h as u64) as u32), even(m)),
        _ => (even(w), even(h)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rates() {
        assert_eq!(reduce(3_000_000_000u64 << 32 | 100_000_000), 30 << 32 | 1);
        assert_eq!(reduce(30_000u64 << 32 | 1001), 30_000 << 32 | 1001);
        assert_eq!(reduce(0), 30 << 32 | 1);
    }

    #[test]
    fn sizes() {
        assert_eq!(scaled(1920, 1080, Some(720)), (1280, 720));
        assert_eq!(scaled(1920, 1080, Some(2000)), (1920, 1080));
        assert_eq!(scaled(853, 481, None), (852, 480));
        assert_eq!(scaled(478, 850, Some(480)), (268, 480));
    }

    #[test]
    fn a_file_windows_cant_read_is_an_error() {
        let p = std::env::temp_dir().join(format!("jay-mf-{}.mp4", std::process::id()));
        std::fs::write(&p, b"not a video").unwrap();
        let out = p.with_extension("out.mp4");
        let r = convert(&p, &out, &ConvertOptions { audio_only: false, max_height: None, video_kbps: 1000, audio_kbps: 128 }, &mut |_| {}, &AtomicBool::new(false));
        assert!(r.is_err());
        assert!(!out.exists());
        std::fs::remove_file(&p).unwrap();
    }
}
