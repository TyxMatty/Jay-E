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

/// The loudest level (0..1) in each of `bins` equal slices of the sound: the timeline's waveform.
/// Decoded to 8 kHz mono (enough to see where it's loud), never held whole. Empty with no sound.
pub fn peaks(src: &Path, bins: usize, cancel: &AtomicBool) -> Result<Vec<f32>, VideoError> {
    const RATE: u32 = 8_000;
    start()?;
    let bins = bins.clamp(1, 4_000);
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(src.as_os_str()), None).map_err(|e| VideoError::Unsupported { what: format!("Windows can't read this file ({})", e.message()) })?;
        if reader.GetNativeMediaType(FIRST_AUDIO, 0).is_err() {
            return Ok(vec![]);
        }
        let duration = reader.GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION).ok().and_then(|v: PROPVARIANT| u64::try_from(&v).ok()).unwrap_or(0).max(1) as f64;
        reader.SetStreamSelection(ALL_STREAMS, false)?;
        reader.SetStreamSelection(FIRST_AUDIO, true)?;
        let pcm = MFCreateMediaType()?;
        pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        pcm.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
        pcm.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
        reader.SetCurrentMediaType(FIRST_AUDIO, None, &pcm).map_err(at("decoding the audio"))?;
        let mut out = vec![0f32; bins];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(VideoError::Cancelled);
            }
            let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
            reader.ReadSample(FIRST_AUDIO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).map_err(at("reading"))?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                break; // what was read so far still shows
            }
            if let Some(sample) = sample {
                let buffer = sample.ConvertToContiguousBuffer()?;
                let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
                buffer.Lock(&mut data, None, Some(&mut len))?;
                let values = std::slice::from_raw_parts(data as *const i16, len as usize / 2);
                for (i, &v) in values.iter().enumerate() {
                    let at = ts as f64 + i as f64 * 1e7 / RATE as f64;
                    let bin = ((at / duration) * bins as f64).clamp(0.0, (bins - 1) as f64) as usize;
                    out[bin] = out[bin].max((v as f32 / 32768.0).abs());
                }
                buffer.Unlock()?;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        Ok(out)
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
        let rgba = sample_rgba(&sample, w, h)?;
        Ok((w, h, rgba, ts as f64 / 1e7))
    }
}

/// A decoded RGB32 frame (`w`×`h`) as RGBA, opaque.
unsafe fn sample_rgba(sample: &IMFSample, w: u32, h: u32) -> Result<Vec<u8>, VideoError> {
    unsafe {
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
        Ok(rgba)
    }
}

/// A sample of our own bytes, for the sink writer.
unsafe fn memory_sample(bytes: &[u8], time: i64, duration: i64) -> Result<IMFSample, VideoError> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
        let mut data = std::ptr::null_mut();
        buffer.Lock(&mut data, None, None)?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
        buffer.Unlock()?;
        buffer.SetCurrentLength(bytes.len() as u32)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(time)?;
        sample.SetSampleDuration(duration)?;
        Ok(sample)
    }
}

/// What an export changes besides the encoding (`ConvertOptions`): the picture as in the image
/// editor, pictures laid on for a while, speed, sound, fades.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Render {
    /// Crop, turns, flips, color (`crate::image::Edits`, applied to every frame).
    pub picture: crate::image::Edits,
    /// Text the page drew, or a logo, shown from one time to another (the video's own seconds).
    pub overlays: Vec<Overlay>,
    /// 0.25..4: faster or slower; the sound's pitch goes with it, as on a tape.
    pub speed: f64,
    /// 0..2 (1: as it was).
    pub volume: f64,
    pub mute: bool,
    /// Seconds fading in from black and silence at the start, and out to them at the end.
    pub fade_in: f64,
    pub fade_out: f64,
}

impl Default for Render {
    fn default() -> Self {
        Self { picture: Default::default(), overlays: vec![], speed: 1.0, volume: 1.0, mute: false, fade_in: 0.0, fade_out: 0.0 }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Overlay {
    pub from: f64,
    pub to: f64,
    pub area: crate::image::Area,
}

impl Render {
    fn speed(&self) -> f64 {
        if self.speed.is_finite() { self.speed.clamp(0.25, 4.0) } else { 1.0 }
    }

    /// The fade (0..1) at `t` seconds of a result `length` seconds long.
    fn fade(&self, t: f64, length: f64) -> f32 {
        let ramp = |d: f64, x: f64| if d.is_finite() && d > 0.0 { (x / d).clamp(0.0, 1.0) } else { 1.0 };
        (ramp(self.fade_in, t) * ramp(self.fade_out, length - t)) as f32
    }

    /// The edits for a frame at `t` seconds of the source: the picture's, and the overlays shown then.
    fn edits_at(&self, t: f64) -> crate::image::Edits {
        let mut edits = self.picture.clone();
        edits.areas.extend(self.overlays.iter().filter(|o| t >= o.from && t < o.to).map(|o| o.area.clone()));
        edits
    }

    /// Some of the sound: the volume, the fades and the speed (by resampling) applied. `pcm` is
    /// 16-bit stereo at 48 kHz starting at `t` seconds of the result.
    fn sound(&self, pcm: &[i16], t: f64, length: f64) -> Vec<i16> {
        let speed = self.speed();
        let frames = pcm.len() / 2;
        let out_frames = ((frames as f64) / speed).round() as usize;
        let volume = if self.mute { 0.0 } else if self.volume.is_finite() { self.volume.clamp(0.0, 2.0) as f32 } else { 1.0 };
        let mut out = Vec::with_capacity(out_frames * 2);
        for k in 0..out_frames {
            let pos = k as f64 * speed;
            let (i, frac) = (pos.floor() as usize, (pos - pos.floor()) as f32);
            let gain = volume * self.fade(t + k as f64 / 48_000.0, length);
            for c in 0..2 {
                let a = pcm.get(i * 2 + c).copied().unwrap_or(0) as f32;
                let b = pcm.get((i + 1).min(frames.saturating_sub(1)) * 2 + c).copied().unwrap_or(0) as f32;
                out.push(((a + (b - a) * frac) * gain).round().clamp(-32768.0, 32767.0) as i16);
            }
        }
        out
    }
}

/// Exports `src` to `dst` (.mp4, or .m4a with `audio_only`) with `edits`: every frame goes through
/// the image editor's edits, the sound through `Render::sound`. Like `convert`, through a
/// temporary file, with progress and cancel.
pub fn render(src: &Path, dst: &Path, options: &ConvertOptions, edits: &Render, progress: &mut dyn FnMut(f64), cancel: &AtomicBool) -> Result<(), VideoError> {
    start()?;
    let ext = if options.audio_only { "m4a" } else { "mp4" };
    let tmp = dst.with_extension(format!("jaye-tmp.{ext}"));
    let result = unsafe { run_render(src, &tmp, options, edits, progress, cancel) };
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

unsafe fn run_render(src: &Path, dst: &Path, options: &ConvertOptions, edits: &Render, progress: &mut dyn FnMut(f64), cancel: &AtomicBool) -> Result<(), VideoError> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(|| VideoError::Unsupported { what: "Media Foundation attributes".into() })?;
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(src.as_os_str()), &attrs).map_err(|e| VideoError::Unsupported { what: format!("Windows can't read this file ({})", e.message()) })?;
        let duration = reader.GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION).ok().and_then(|v: PROPVARIANT| u64::try_from(&v).ok()).unwrap_or(0).max(1) as f64 / 1e7;
        let speed = edits.speed();
        let length = duration / speed;
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

        // The video: decoded (scaled) to RGB32, edited, written as RGB32 the writer encodes.
        let mut video = None; // (writer stream, decoded size, output size, frame seconds)
        if has_video {
            reader.SetStreamSelection(FIRST_VIDEO, true)?;
            let native = reader.GetNativeMediaType(FIRST_VIDEO, 0)?;
            let size = native.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let (w, h) = scaled((size >> 32) as u32, size as u32, options.max_height);
            let rate = reduce(native.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(30u64 << 32 | 1));
            let fps = (rate >> 32) as f64 / (rate & 0xffff_ffff).max(1) as f64;
            let rgb = MFCreateMediaType()?;
            rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            rgb.SetUINT64(&MF_MT_FRAME_SIZE, (w as u64) << 32 | h as u64)?;
            reader.SetCurrentMediaType(FIRST_VIDEO, None, &rgb).map_err(at("decoding the video"))?;
            // The edited size, made even (H.264 wants it).
            let (ow, oh) = edits.picture.size(w, h);
            let (ow, oh) = ((ow & !1).max(2), (oh & !1).max(2));
            let h264 = MFCreateMediaType()?;
            h264.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            h264.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            h264.SetUINT32(&MF_MT_AVG_BITRATE, options.video_kbps.clamp(100, 100_000) * 1000)?;
            h264.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            h264.SetUINT64(&MF_MT_FRAME_SIZE, (ow as u64) << 32 | oh as u64)?;
            h264.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
            h264.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1u64 << 32 | 1)?;
            let index = writer.AddStream(&h264).map_err(at("setting up the H.264 encoder"))?;
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, (ow as u64) << 32 | oh as u64)?;
            input.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1u64 << 32 | 1)?;
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, ow * 4)?;
            writer.SetInputMediaType(index, &input, None).map_err(at("connecting the H.264 encoder"))?;
            video = Some((index, (w, h), (ow, oh), 1.0 / fps.max(1.0)));
        }
        let mut audio = None;
        if has_audio {
            reader.SetStreamSelection(FIRST_AUDIO, true)?;
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
            audio = Some(index);
        }

        writer.BeginWriting().map_err(at("starting to write"))?;
        // Read from whichever stream is behind (source time), so the file comes out interleaved.
        let streams: Vec<u32> = [video.map(|_| FIRST_VIDEO), audio.map(|_| FIRST_AUDIO)].into_iter().flatten().collect();
        let mut last_ts = vec![0i64; streams.len()];
        let mut done = vec![false; streams.len()];
        let mut base: Option<i64> = None;
        let mut next_frame = 0.0; // the result's time the next kept frame may have
        let mut reported = -1.0;
        let mut bgra = Vec::new();
        while let Some(i) = (0..streams.len()).filter(|&i| !done[i]).min_by_key(|&i| last_ts[i]) {
            if cancel.load(Ordering::Relaxed) {
                return Err(VideoError::Cancelled);
            }
            let stream = streams[i];
            let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
            reader.ReadSample(stream, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).map_err(at("reading"))?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(VideoError::Invalid { message: "the file stopped decoding".into() });
            }
            if let Some(sample) = sample.filter(|s| s.GetTotalLength().unwrap_or(0) > 0) {
                let base = *base.get_or_insert(ts);
                let source_t = (ts - base).max(0) as f64 / 1e7;
                let out_t = source_t / speed;
                if Some(stream) == video.map(|_| FIRST_VIDEO) {
                    let (index, (w, h), (ow, oh), frame) = video.unwrap();
                    // Faster: frames that would come sooner than one per source frame are left out.
                    if out_t + 1e-6 >= next_frame {
                        next_frame = out_t + frame * 0.9;
                        let rgba = sample_rgba(&sample, w, h)?;
                        let picture = image::RgbaImage::from_raw(w, h, rgba).ok_or_else(|| VideoError::Invalid { message: "frame size".into() })?;
                        let edited = crate::image::apply(&image::DynamicImage::ImageRgba8(picture), &edits.edits_at(source_t)).into_rgba8();
                        let fade = edits.fade(out_t, length);
                        bgra.clear();
                        bgra.reserve((ow * oh * 4) as usize);
                        for y in 0..oh {
                            for x in 0..ow {
                                let p = edited.get_pixel(x.min(edited.width() - 1), y.min(edited.height() - 1)).0;
                                bgra.extend([(p[2] as f32 * fade) as u8, (p[1] as f32 * fade) as u8, (p[0] as f32 * fade) as u8, 255]);
                            }
                        }
                        let out = memory_sample(&bgra, (out_t * 1e7) as i64, (frame * 1e7) as i64)?;
                        writer.WriteSample(index, &out).map_err(at("encoding"))?;
                    }
                } else if let Some(index) = audio {
                    let buffer = sample.ConvertToContiguousBuffer()?;
                    let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
                    buffer.Lock(&mut data, None, Some(&mut len))?;
                    let pcm = std::slice::from_raw_parts(data as *const i16, len as usize / 2).to_vec();
                    buffer.Unlock()?;
                    let sound = edits.sound(&pcm, out_t, length);
                    if !sound.is_empty() {
                        let bytes = std::slice::from_raw_parts(sound.as_ptr() as *const u8, sound.len() * 2);
                        let out = memory_sample(bytes, (out_t * 1e7) as i64, (sound.len() as f64 / 2.0 / 48_000.0 * 1e7) as i64)?;
                        writer.WriteSample(index, &out).map_err(at("encoding"))?;
                    }
                }
                last_ts[i] = ts;
            } else if flags & MF_SOURCE_READERF_STREAMTICK.0 as u32 != 0 {
                last_ts[i] = ts;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                done[i] = true;
            }
            let p = ((last_ts.iter().copied().min().unwrap_or(0) - base.unwrap_or(0)) as f64 / 1e7 / duration).clamp(0.0, 1.0);
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
    fn render_sound_and_fades() {
        let r = Render { speed: 2.0, volume: 0.5, fade_in: 1.0, fade_out: 1.0, ..Default::default() };
        // Fades: black at the ends, full in the middle.
        assert_eq!((r.fade(0.0, 10.0), r.fade(0.5, 10.0), r.fade(5.0, 10.0), r.fade(10.0, 10.0)), (0.0, 0.5, 1.0, 0.0));
        // Twice as fast: half the samples; half the volume.
        let pcm: Vec<i16> = (0..2000).map(|i| if i % 2 == 0 { 10_000 } else { -10_000 }).collect();
        let out = r.sound(&pcm, 5.0, 10.0);
        assert_eq!(out.len(), 1000);
        assert_eq!((out[0], out[1]), (5000, -5000));
        // Muted: silence. Slower: more samples.
        assert!(Render { mute: true, ..Default::default() }.sound(&pcm, 0.0, 10.0).iter().all(|&v| v == 0));
        assert_eq!(Render { speed: 0.5, ..Default::default() }.sound(&pcm, 0.0, 10.0).len(), 4000);
        // Speed kept between 0.25 and 4; an older request without the fields is "as it was".
        assert_eq!(Render { speed: 100.0, ..Default::default() }.speed(), 4.0);
        let r: Render = serde_json::from_str("{}").unwrap();
        assert_eq!((r.speed, r.volume, r.mute), (1.0, 1.0, false));
    }

    /// A real export: needs a video with sound, named by JAYE_TEST_VIDEO (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn render_a_real_video() {
        let Some(src) = std::env::var_os("JAYE_TEST_VIDEO").map(std::path::PathBuf::from) else { return };
        let (seconds, _, _) = probe(&src).unwrap();
        let dst = std::env::temp_dir().join(format!("jaye-render-{}.mp4", std::process::id()));
        // A white box laid on from 1 s to 3 s, as the page's text would be.
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(8, 4, image::Rgba([255, 255, 255, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let b64: String = {
            const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            png.chunks(3).flat_map(|c| {
                let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
                (0..4).map(move |i| if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' })
            }).collect()
        };
        let area = crate::image::Area { at: 2, x: 0.25, y: 0.4, w: 0.5, h: 0.2, kind: crate::image::AreaKind::Stamp { png: b64 } };
        let edits = Render {
            picture: crate::image::Edits { ops: vec![crate::image::Op::RotateRight, crate::image::Op::Crop { x: 0.0, y: 0.2, w: 1.0, h: 0.6 }], ..Default::default() },
            overlays: vec![Overlay { from: 1.0, to: 3.0, area }],
            speed: 2.0,
            fade_in: 0.5,
            fade_out: 0.5,
            ..Default::default()
        };
        let options = ConvertOptions { audio_only: false, max_height: Some(480), video_kbps: 1500, audio_kbps: 128 };
        let started = std::time::Instant::now();
        render(&src, &dst, &options, &edits, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        let (out_seconds, video, audio) = probe(&dst).unwrap();
        eprintln!("RENDER {seconds:.2}s → {out_seconds:.2}s in {:?}", started.elapsed());
        assert!(video && audio);
        assert!((out_seconds - seconds / 2.0).abs() < 0.3, "{out_seconds}");
        // The first frame is black (fade in); at 1 s of the result (2 s of the source) the box shows.
        let (w, h, first, _) = frame(&dst, 0.0).unwrap();
        assert!(first.chunks(4).all(|p| p[0] < 40 && p[1] < 40 && p[2] < 40));
        let (_, _, mid, _) = frame(&dst, 1.0).unwrap();
        let center = &mid[((h as usize / 2) * w as usize + w as usize / 2) * 4..][..3];
        assert!(center.iter().all(|&v| v > 200), "{center:?}");
        eprintln!("RENDER size {w}×{h}");
        let _ = std::fs::remove_file(&dst);
    }

    #[test]
    fn overlays_show_only_in_their_time() {
        let area = crate::image::Area { at: 0, x: 0.1, y: 0.1, w: 0.2, h: 0.2, kind: crate::image::AreaKind::Blur };
        let r = Render { overlays: vec![Overlay { from: 2.0, to: 4.0, area }], ..Default::default() };
        assert!(r.edits_at(1.0).areas.is_empty() && r.edits_at(3.0).areas.len() == 1 && r.edits_at(4.0).areas.is_empty());
    }

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
