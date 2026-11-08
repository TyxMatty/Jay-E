//! Converting video with Media Foundation (Windows): any format Windows can play in, MP4 (H.264 +
//! AAC) or M4A (AAC only) out. The source reader decodes (and scales, through its video processor)
//! and the sink writer encodes, both with hardware transforms when there are any. Decision D3: 0 MB
//! in the installer; WebM/VP9 input needs the "Web Media Extensions" Windows ships with.

use super::VideoError;
use std::io::Write;
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

/// What an export changes besides the encoding (`ConvertOptions`): which pieces, in which order
/// (and backward), the picture as in the image editor, pictures laid on for a while, speed, sound,
/// music, fades and dissolves.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Render {
    /// Crop, turns, flips, color (`crate::image::Edits`, applied to every frame).
    pub picture: crate::image::Edits,
    /// Text the page drew, a logo, a blurred part: shown from one time to another (the video's
    /// own seconds).
    pub overlays: Vec<Overlay>,
    /// 0.25..4: faster or slower; the sound's pitch goes with it, as on a tape.
    pub speed: f64,
    /// 0..2 (1: as it was).
    pub volume: f64,
    pub mute: bool,
    /// Seconds fading in from black and silence at the start, and out to them at the end.
    pub fade_in: f64,
    pub fade_out: f64,
    /// The razor's pieces to keep, in the order they play (`[start, end]` seconds); empty: all.
    pub pieces: Vec<[f64; 2]>,
    /// Played backward: the pieces in reverse order, each from its end to its start.
    pub reverse: bool,
    /// Seconds each piece dissolves into the next (0: a plain cut). At most 3, and a third of the
    /// shortest piece.
    pub dissolve: f64,
    /// Music mixed in, or instead of the video's own sound.
    pub music: Option<Music>,
}

impl Default for Render {
    fn default() -> Self {
        Self { picture: Default::default(), overlays: vec![], speed: 1.0, volume: 1.0, mute: false, fade_in: 0.0, fade_out: 0.0, pieces: vec![], reverse: false, dissolve: 0.0, music: None }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Overlay {
    pub from: f64,
    pub to: f64,
    pub area: crate::image::Area,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct Music {
    /// A file the person picked (the app checks).
    pub path: String,
    /// 0..2.
    pub volume: f64,
    /// Instead of the video's sound (otherwise mixed with it).
    pub replace: bool,
}

impl Default for Music {
    fn default() -> Self {
        Self { path: String::new(), volume: 1.0, replace: false }
    }
}

/// A stretch of the source in the result: `start` to `end` seconds, maybe played backward.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Span {
    start: f64,
    end: f64,
    backward: bool,
}

/// Reversing keeps a whole piece's sound in memory: longer pieces are refused.
const MAX_REVERSE_SECONDS: f64 = 20.0 * 60.0;
/// A GIF is for a short moment.
const MAX_GIF_SECONDS: f64 = 60.0;

impl Render {
    fn speed(&self) -> f64 {
        if self.speed.is_finite() { self.speed.clamp(0.25, 4.0) } else { 1.0 }
    }

    fn gain(&self) -> f32 {
        if self.mute || self.music.as_ref().is_some_and(|m| m.replace) {
            0.0
        } else if self.volume.is_finite() {
            self.volume.clamp(0.0, 2.0) as f32
        } else {
            1.0
        }
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

    /// The source's stretches, in the order they play.
    fn spans(&self, duration: f64) -> Vec<Span> {
        let mut spans: Vec<Span> = if self.pieces.is_empty() {
            vec![Span { start: 0.0, end: duration, backward: false }]
        } else {
            self.pieces
                .iter()
                .filter(|p| p[0].is_finite() && p[1].is_finite())
                .map(|p| Span { start: p[0].clamp(0.0, duration), end: p[1].clamp(0.0, duration), backward: false })
                .filter(|s| s.end - s.start > 0.05)
                .collect()
        };
        if self.reverse {
            spans.reverse();
            spans.iter_mut().for_each(|s| s.backward = true);
        }
        spans
    }

    /// Seconds two neighbouring pieces overlap: the dissolve, held to 3 s and a third of the
    /// shortest piece.
    fn overlap(&self, spans: &[Span]) -> f64 {
        if spans.len() < 2 || !self.dissolve.is_finite() {
            return 0.0;
        }
        let shortest = spans.iter().map(|s| (s.end - s.start) / self.speed()).fold(f64::MAX, f64::min);
        self.dissolve.clamp(0.0, 3.0).min(shortest / 3.0)
    }

    /// Where each piece starts in the result, and the result's length.
    fn layout(&self, spans: &[Span]) -> (Vec<f64>, f64) {
        let d = self.overlap(spans);
        let mut at = 0.0;
        let starts = spans
            .iter()
            .map(|s| {
                let start = at;
                at += (s.end - s.start) / self.speed() - d;
                start
            })
            .collect();
        (starts, at + if spans.is_empty() { 0.0 } else { d })
    }

    /// Some of the sound: the volume, the fades and the speed (by resampling) applied. `pcm` is
    /// 16-bit stereo at 48 kHz starting at `t` seconds of the result.
    fn sound(&self, pcm: &[i16], t: f64, length: f64) -> Vec<i16> {
        let speed = self.speed();
        let frames = pcm.len() / 2;
        let out_frames = ((frames as f64) / speed).round() as usize;
        let volume = self.gain();
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

/// The pieces' meeting points: what a span sends out comes in order and is either written, held
/// back (the end that dissolves into the next piece) or mixed with what was held (its start).
struct Crossfade<T> {
    /// Seconds of overlap.
    d: f64,
    held: Vec<(f64, T)>,
    next_held: Vec<(f64, T)>,
}

impl<T> Crossfade<T> {
    fn new(d: f64) -> Self {
        Self { d, held: vec![], next_held: vec![] }
    }

    /// For something at `local` seconds of span `k` (of `count`, `length` long): whether to hold it
    /// (the end of a piece that dissolves), or the held one to mix it with and how much of the new.
    fn place(&mut self, k: usize, count: usize, local: f64, length: f64) -> Place {
        if self.d <= 0.0 {
            return Place::Write;
        }
        if k + 1 < count && local >= length - self.d {
            return Place::Hold;
        }
        if k > 0 && local < self.d && !self.held.is_empty() {
            // The held item closest in time: the old piece at (its length - d + local).
            let i = ((local / self.d) * self.held.len() as f64).floor() as usize;
            return Place::Mix(i.min(self.held.len() - 1), (local / self.d) as f32);
        }
        Place::Write
    }

    /// A span ended: what it held becomes what the next one mixes with.
    fn next_span(&mut self) {
        self.held = std::mem::take(&mut self.next_held);
    }
}

enum Place {
    Write,
    Hold,
    /// The held item to mix with, and how much of the new one (0..1).
    Mix(usize, f32),
}

/// A source reader for one stream (video or audio), with seeking.
unsafe fn open_reader(src: &Path, video: bool) -> Result<IMFSourceReader, VideoError> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(|| VideoError::Unsupported { what: "Media Foundation attributes".into() })?;
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(src.as_os_str()), &attrs).map_err(|e| VideoError::Unsupported { what: format!("Windows can't read this file ({})", e.message()) })?;
        reader.SetStreamSelection(ALL_STREAMS, false)?;
        reader.SetStreamSelection(if video { FIRST_VIDEO } else { FIRST_AUDIO }, true)?;
        Ok(reader)
    }
}

unsafe fn seek(reader: &IMFSourceReader, seconds: f64) -> Result<(), VideoError> {
    unsafe { reader.SetCurrentPosition(&windows::core::GUID::zeroed(), &PROPVARIANT::from((seconds.max(0.0) * 1e7) as i64)).map_err(at("seeking")) }
}

/// The audio stream decoded to 16-bit stereo PCM at 48 kHz.
unsafe fn pcm_reader(src: &Path) -> Result<Option<IMFSourceReader>, VideoError> {
    unsafe {
        let reader = open_reader(src, false)?;
        if reader.GetNativeMediaType(FIRST_AUDIO, 0).is_err() {
            return Ok(None);
        }
        let pcm = MFCreateMediaType()?;
        pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        pcm.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
        pcm.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        reader.SetCurrentMediaType(FIRST_AUDIO, None, &pcm).map_err(at("decoding the audio"))?;
        Ok(Some(reader))
    }
}

/// The next decoded audio: (seconds, PCM), or None at the end.
unsafe fn next_pcm(reader: &IMFSourceReader) -> Result<Option<(f64, Vec<i16>)>, VideoError> {
    unsafe {
        loop {
            let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
            reader.ReadSample(FIRST_AUDIO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).map_err(at("reading"))?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(VideoError::Invalid { message: "the sound stopped decoding".into() });
            }
            if let Some(sample) = sample {
                let buffer = sample.ConvertToContiguousBuffer()?;
                let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
                buffer.Lock(&mut data, None, Some(&mut len))?;
                let pcm = std::slice::from_raw_parts(data as *const i16, len as usize / 2).to_vec();
                buffer.Unlock()?;
                if !pcm.is_empty() {
                    return Ok(Some((ts as f64 / 1e7, pcm)));
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Ok(None);
            }
        }
    }
}

/// The span's sound, in the order it plays: read from `start` to `end` (cut to them), backward if
/// the span is; handed out in pieces with their seconds from the span's start (source seconds).
unsafe fn span_pcm(reader: &IMFSourceReader, span: Span, cancel: &AtomicBool, out: &mut dyn FnMut(f64, &[i16]) -> Result<(), VideoError>) -> Result<(), VideoError> {
    unsafe {
        seek(reader, span.start)?;
        let mut whole = Vec::new(); // backward spans, whole
        while let Some((t0, pcm)) = next_pcm(reader)? {
            if cancel.load(Ordering::Relaxed) {
                return Err(VideoError::Cancelled);
            }
            let frames = pcm.len() / 2;
            let t1 = t0 + frames as f64 / 48_000.0;
            if t1 <= span.start {
                continue;
            }
            if t0 >= span.end {
                break;
            }
            let skip = (((span.start - t0) * 48_000.0).round().max(0.0) as usize).min(frames);
            let keep = ((((span.end - t0) * 48_000.0).round().max(0.0)) as usize).min(frames);
            let part = &pcm[skip * 2..keep.max(skip) * 2];
            if span.backward {
                if whole.len() as f64 / 96_000.0 > MAX_REVERSE_SECONDS {
                    return Err(VideoError::Unsupported { what: "playing backward: pieces up to 20 minutes".into() });
                }
                whole.extend_from_slice(part);
            } else {
                out((t0 + skip as f64 / 48_000.0 - span.start).max(0.0), part)?;
            }
        }
        if span.backward {
            // Frame by frame from the end (each frame keeps its left and right).
            let reversed: Vec<i16> = whole.chunks_exact(2).rev().flatten().copied().collect();
            for (i, chunk) in reversed.chunks(9_600).enumerate() {
                out(i as f64 * 4_800.0 / 48_000.0, chunk)?;
            }
        }
        Ok(())
    }
}

/// Music decoded as it's needed, mixed into the sound in order.
struct MusicFeed {
    reader: IMFSourceReader,
    queue: std::collections::VecDeque<i16>,
    done: bool,
    volume: f32,
}

impl MusicFeed {
    /// The next `n` values (`n / 2` stereo frames), silence after the music ends.
    unsafe fn take(&mut self, n: usize) -> Result<Vec<i16>, VideoError> {
        while self.queue.len() < n && !self.done {
            match unsafe { next_pcm(&self.reader)? } {
                Some((_, pcm)) => self.queue.extend(pcm),
                None => self.done = true,
            }
        }
        Ok((0..n).map(|_| self.queue.pop_front().unwrap_or(0)).collect())
    }
}

/// The result's whole sound, written as raw PCM to `out` (it plays from 0 to `length` seconds):
/// each span's sound with speed, volume and fades, the dissolves crossfaded, the music mixed in.
/// False when there's no sound at all.
unsafe fn build_sound(src: &Path, edits: &Render, spans: &[Span], starts: &[f64], length: f64, out: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(f64)) -> Result<bool, VideoError> {
    unsafe {
        let main = pcm_reader(src)?;
        let mut music = match &edits.music {
            Some(m) if !m.path.is_empty() => pcm_reader(Path::new(&m.path))?.map(|reader| MusicFeed { reader, queue: Default::default(), done: false, volume: if m.volume.is_finite() { m.volume.clamp(0.0, 2.0) as f32 } else { 1.0 } }),
            _ => None,
        };
        if main.is_none() && music.is_none() {
            return Ok(false);
        }
        let mut file = std::io::BufWriter::new(std::fs::File::create(out)?);
        let written = std::cell::Cell::new(0usize); // stereo frames
        let mut write = |pcm: &[i16], music: &mut Option<MusicFeed>| -> Result<(), VideoError> {
            let mut mixed = pcm.to_vec();
            if let Some(m) = music.as_mut() {
                for (v, s) in mixed.iter_mut().zip(m.take(pcm.len())?) {
                    *v = (*v as f32 + s as f32 * m.volume).round().clamp(-32768.0, 32767.0) as i16;
                }
            }
            for v in &mixed {
                file.write_all(&v.to_le_bytes())?;
            }
            written.set(written.get() + mixed.len() / 2);
            Ok(())
        };
        let speed = edits.speed();
        let mut fade = Crossfade::<[i16; 2]>::new(edits.overlap(spans));
        for (k, span) in spans.iter().enumerate() {
            let span_length = (span.end - span.start) / speed;
            let mut handle = |local_src: f64, pcm: &[i16]| -> Result<(), VideoError> {
                let local = local_src / speed;
                let processed = edits.sound(pcm, starts[k] + local, length);
                let mut direct = Vec::with_capacity(processed.len());
                for (i, f) in processed.chunks_exact(2).enumerate() {
                    let at = local + i as f64 / 48_000.0;
                    match fade.place(k, spans.len(), at, span_length) {
                        Place::Write => direct.extend_from_slice(f),
                        Place::Hold => fade.next_held.push((at, [f[0], f[1]])),
                        Place::Mix(j, a) => {
                            let old = fade.held[j].1;
                            direct.extend((0..2).map(|c| (old[c] as f32 * (1.0 - a) + f[c] as f32 * a).round() as i16));
                        }
                    }
                }
                write(&direct, &mut music)
            };
            match &main {
                Some(reader) => span_pcm(reader, *span, cancel, &mut handle)?,
                // No sound of its own: silence the length of the span (the music fills it).
                None => {
                    let frames = (span_length * speed * 48_000.0) as usize;
                    for chunk in 0..frames.div_ceil(4_800) {
                        let n = 4_800.min(frames - chunk * 4_800);
                        handle(chunk as f64 * 0.1, &vec![0; n * 2])?;
                    }
                }
            }
            fade.next_span();
            progress((k + 1) as f64 / spans.len() as f64);
        }
        // Sound shorter than the picture (a dropped tail): silence to the end.
        let total = (length * 48_000.0) as usize;
        if written.get() < total {
            write(&vec![0; (total - written.get()) * 2], &mut music)?;
        }
        file.flush()?;
        Ok(true)
    }
}

/// Frames of the source for a span, in the order they play, each with its source seconds; a
/// backward span keeps them (JPEG) in a temporary file, then gives them from the last.
unsafe fn span_frames(reader: &IMFSourceReader, span: Span, size: (u32, u32), store: &Path, cancel: &AtomicBool, out: &mut dyn FnMut(f64, image::RgbaImage) -> Result<(), VideoError>) -> Result<(), VideoError> {
    unsafe {
        let (w, h) = size;
        seek(reader, span.start)?;
        let mut kept: Vec<(f64, u64, usize)> = vec![]; // backward: (seconds, offset, length)
        let mut file = if span.backward { Some(std::io::BufWriter::new(std::fs::File::create(store)?)) } else { None };
        let mut offset = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(VideoError::Cancelled);
            }
            let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
            reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).map_err(at("reading"))?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(VideoError::Invalid { message: "the file stopped decoding".into() });
            }
            if let Some(sample) = sample.filter(|s| s.GetTotalLength().unwrap_or(0) > 0) {
                let t = ts as f64 / 1e7;
                if t >= span.end {
                    break;
                }
                if t + 1e-4 >= span.start {
                    let picture = image::RgbaImage::from_raw(w, h, sample_rgba(&sample, w, h)?).ok_or_else(|| VideoError::Invalid { message: "frame size".into() })?;
                    match file.as_mut() {
                        None => out(t, picture)?,
                        Some(f) => {
                            let mut jpeg = Vec::new();
                            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 92).encode_image(&image::DynamicImage::ImageRgba8(picture).to_rgb8()).map_err(|e| VideoError::Io { message: e.to_string() })?;
                            f.write_all(&jpeg)?;
                            kept.push((t, offset, jpeg.len()));
                            offset += jpeg.len() as u64;
                        }
                    }
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        if let Some(f) = file {
            drop(f);
            let mut f = std::fs::File::open(store)?;
            for &(t, at, len) in kept.iter().rev() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(VideoError::Cancelled);
                }
                let mut jpeg = vec![0; len];
                std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(at))?;
                std::io::Read::read_exact(&mut f, &mut jpeg)?;
                let picture = image::load_from_memory(&jpeg).map_err(|e| VideoError::Io { message: e.to_string() })?.to_rgba8();
                out(t, picture)?;
            }
            let _ = std::fs::remove_file(store);
        }
        Ok(())
    }
}

/// Every frame of the result, in order: each span's frames edited (`Render::edits_at`), the faster
/// ones thinned to the source's frame rate, dissolves blended, fades applied; handed to `out` with
/// its seconds in the result.
#[allow(clippy::too_many_arguments)]
unsafe fn result_frames(src: &Path, edits: &Render, spans: &[Span], starts: &[f64], length: f64, max_height: Option<u32>, store: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(f64), out: &mut dyn FnMut(f64, &image::RgbaImage) -> Result<(), VideoError>) -> Result<(), VideoError> {
    unsafe {
        let reader = open_reader(src, true)?;
        let native = reader.GetNativeMediaType(FIRST_VIDEO, 0)?;
        let size = native.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (w, h) = scaled((size >> 32) as u32, size as u32, max_height);
        let rate = reduce(native.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(30u64 << 32 | 1));
        let frame = (rate & 0xffff_ffff).max(1) as f64 / (rate >> 32).max(1) as f64;
        let rgb = MFCreateMediaType()?;
        rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        rgb.SetUINT64(&MF_MT_FRAME_SIZE, (w as u64) << 32 | h as u64)?;
        reader.SetCurrentMediaType(FIRST_VIDEO, None, &rgb).map_err(at("decoding the video"))?;
        let speed = edits.speed();
        let mut fade = Crossfade::<Vec<u8>>::new(edits.overlap(spans)); // held frames as JPEG
        for (k, span) in spans.iter().enumerate() {
            let span_length = (span.end - span.start) / speed;
            let mut next_frame = 0.0;
            span_frames(&reader, *span, (w, h), store, cancel, &mut |t, picture| {
                let local = if span.backward { span.end - t } else { t - span.start } / speed;
                // Faster: frames that would come sooner than one source frame apart are left out.
                if local + 1e-6 < next_frame {
                    return Ok(());
                }
                next_frame = local + frame * 0.9;
                let mut edited = crate::image::apply(&image::DynamicImage::ImageRgba8(picture), &edits.edits_at(t)).into_rgba8();
                match fade.place(k, spans.len(), local, span_length) {
                    Place::Hold => {
                        let mut jpeg = Vec::new();
                        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 92).encode_image(&image::DynamicImage::ImageRgba8(edited).to_rgb8()).map_err(|e| VideoError::Io { message: e.to_string() })?;
                        fade.next_held.push((local, jpeg));
                        return Ok(());
                    }
                    Place::Mix(j, a) => {
                        let old = image::load_from_memory(&fade.held[j].1).map_err(|e| VideoError::Io { message: e.to_string() })?.to_rgba8();
                        if old.dimensions() == edited.dimensions() {
                            for (n, o) in edited.pixels_mut().zip(old.pixels()) {
                                for c in 0..3 {
                                    n[c] = (o[c] as f32 * (1.0 - a) + n[c] as f32 * a).round() as u8;
                                }
                            }
                        }
                    }
                    Place::Write => {}
                }
                let at = starts[k] + local;
                let f = edits.fade(at, length);
                if f < 1.0 {
                    for p in edited.pixels_mut() {
                        for c in 0..3 {
                            p[c] = (p[c] as f32 * f) as u8;
                        }
                    }
                }
                progress((at / length).clamp(0.0, 1.0));
                out(at, &edited)
            })?;
            fade.next_span();
        }
        Ok(())
    }
}

/// Exports `src` to `dst` (.mp4, or .m4a with `audio_only`) with `edits`. Like `convert`, through
/// a temporary file, with progress and cancel.
pub fn render(src: &Path, dst: &Path, options: &ConvertOptions, edits: &Render, progress: &mut dyn FnMut(f64), cancel: &AtomicBool) -> Result<(), VideoError> {
    start()?;
    let ext = if options.audio_only { "m4a" } else { "mp4" };
    let tmp = dst.with_extension(format!("jaye-tmp.{ext}"));
    let sound = dst.with_extension("jaye-sound.pcm");
    let store = dst.with_extension("jaye-frames.tmp");
    let result = unsafe { run_render(src, &tmp, &sound, &store, options, edits, progress, cancel) };
    let _ = std::fs::remove_file(&sound);
    let _ = std::fs::remove_file(&store);
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

#[allow(clippy::too_many_arguments)]
unsafe fn run_render(src: &Path, dst: &Path, sound_file: &Path, store: &Path, options: &ConvertOptions, edits: &Render, progress: &mut dyn FnMut(f64), cancel: &AtomicBool) -> Result<(), VideoError> {
    unsafe {
        let (duration, has_video, _) = probe(src)?;
        let has_video = has_video && !options.audio_only;
        let spans = edits.spans(duration);
        if spans.is_empty() {
            return Err(VideoError::Invalid { message: "no pieces to keep".into() });
        }
        let (starts, length) = edits.layout(&spans);
        // The sound first (it's small), then the frames with the sound written alongside.
        let has_sound = build_sound(src, edits, &spans, &starts, length, sound_file, cancel, &mut |p| progress(p * 0.15))?;
        if !has_video && !has_sound {
            return Err(VideoError::Unsupported { what: "no video or audio Windows can decode".into() });
        }
        let mut writer_attrs = None;
        MFCreateAttributes(&mut writer_attrs, 1)?;
        let writer_attrs = writer_attrs.ok_or_else(|| VideoError::Unsupported { what: "Media Foundation attributes".into() })?;
        writer_attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(dst.as_os_str()), None, &writer_attrs)?;
        let mut video_stream = None; // (index, output size)
        if has_video {
            let reader = open_reader(src, true)?;
            let native = reader.GetNativeMediaType(FIRST_VIDEO, 0)?;
            let size = native.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let (w, h) = scaled((size >> 32) as u32, size as u32, options.max_height);
            let rate = reduce(native.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(30u64 << 32 | 1));
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
            video_stream = Some((index, (ow, oh), (rate & 0xffff_ffff).max(1) as f64 / (rate >> 32).max(1) as f64));
        }
        let mut audio_stream = None;
        if has_sound {
            let pcm = MFCreateMediaType()?;
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            pcm.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
            pcm.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            pcm.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
            pcm.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192_000)?;
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
            writer.SetInputMediaType(index, &pcm, None).map_err(at("connecting the AAC encoder"))?;
            audio_stream = Some(index);
        }
        writer.BeginWriting().map_err(at("starting to write"))?;
        // The sound goes in tenth-of-a-second pieces, kept a little ahead of the picture.
        let mut sound = if has_sound { Some(std::io::BufReader::new(std::fs::File::open(sound_file)?)) } else { None };
        let mut sound_at = 0usize; // stereo frames written
        let mut write_sound_until = |until: f64| -> Result<(), VideoError> {
            let (Some(index), Some(reader)) = (audio_stream, sound.as_mut()) else { return Ok(()) };
            while (sound_at as f64) / 48_000.0 < until {
                let mut bytes = vec![0u8; 4_800 * 4];
                let mut got = 0;
                while got < bytes.len() {
                    match std::io::Read::read(reader, &mut bytes[got..])? {
                        0 => break,
                        n => got += n,
                    }
                }
                if got < 4 {
                    return Ok(());
                }
                let frames = got / 4;
                let sample = memory_sample(&bytes[..frames * 4], (sound_at as f64 / 48_000.0 * 1e7) as i64, (frames as f64 / 48_000.0 * 1e7) as i64)?;
                writer.WriteSample(index, &sample).map_err(at("encoding"))?;
                sound_at += frames;
            }
            Ok(())
        };
        if let Some((index, (ow, oh), frame)) = video_stream {
            let mut bgra = Vec::new();
            result_frames(src, edits, &spans, &starts, length, options.max_height, store, cancel, &mut |p| progress(0.15 + p * 0.85), &mut |time, picture| {
                write_sound_until(time + 0.5)?;
                bgra.clear();
                bgra.reserve((ow * oh * 4) as usize);
                for y in 0..oh {
                    for x in 0..ow {
                        let p = picture.get_pixel(x.min(picture.width() - 1), y.min(picture.height() - 1)).0;
                        bgra.extend([p[2], p[1], p[0], 255]);
                    }
                }
                let sample = memory_sample(&bgra, (time * 1e7) as i64, (frame * 1e7) as i64)?;
                writer.WriteSample(index, &sample).map_err(at("encoding"))?;
                Ok(())
            })?;
        }
        write_sound_until(f64::MAX)?;
        writer.Finalize().map_err(at("finishing the file"))?;
        progress(1.0);
        Ok(())
    }
}

/// An animated GIF of the result (no sound): at most `max_width` wide and `fps` frames a second,
/// up to a minute. Through a temporary file, like `render`.
pub fn render_gif(src: &Path, dst: &Path, edits: &Render, max_width: u32, fps: u32, progress: &mut dyn FnMut(f64), cancel: &AtomicBool) -> Result<(), VideoError> {
    start()?;
    let tmp = dst.with_extension("jaye-tmp.gif");
    let store = dst.with_extension("jaye-frames.tmp");
    let result = (|| unsafe {
        let (duration, has_video, _) = probe(src)?;
        if !has_video {
            return Err(VideoError::Unsupported { what: "no video to make a GIF of".into() });
        }
        let spans = edits.spans(duration);
        let (starts, length) = edits.layout(&spans);
        if spans.is_empty() || length > MAX_GIF_SECONDS {
            return Err(VideoError::Unsupported { what: "a GIF of up to a minute: mark a shorter part (or cut first)".into() });
        }
        // The decoded height that keeps the width within `max_width`.
        let reader = open_reader(src, true)?;
        let size = reader.GetNativeMediaType(FIRST_VIDEO, 0)?.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (w, h) = ((size >> 32) as u32, size as u32);
        let max_height = (max_width as u64 * h as u64 / w.max(1) as u64) as u32;
        let file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(file, 10);
        encoder.set_repeat(image::codecs::gif::Repeat::Infinite).map_err(|e| VideoError::Io { message: e.to_string() })?;
        let step = 1.0 / fps.clamp(1, 30) as f64;
        let mut next = 0.0;
        result_frames(src, edits, &spans, &starts, length, Some(max_height.max(2)), &store, cancel, progress, &mut |at, picture| {
            if at + 1e-6 < next {
                return Ok(());
            }
            // On a fixed grid (frames rarely land on it exactly; stepping from each one would drift).
            next += step * ((at + 1e-6 - next) / step).floor().max(0.0) + step;
            let delay = image::Delay::from_numer_denom_ms((step * 1000.0).round() as u32, 1);
            encoder.encode_frame(image::Frame::from_parts(picture.clone(), 0, 0, delay)).map_err(|e| VideoError::Io { message: e.to_string() })
        })?;
        drop(encoder);
        Ok(())
    })();
    let _ = std::fs::remove_file(&store);
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

/// A PNG of a frame every `every` seconds into `dir` (named after the video and the time, never
/// over an existing file). Returns how many were saved.
pub fn frames_every(src: &Path, every: f64, dir: &Path, progress: &mut dyn FnMut(f64), cancel: &AtomicBool) -> Result<usize, VideoError> {
    start()?;
    let every = if every.is_finite() { every.clamp(0.1, 3_600.0) } else { 1.0 };
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "video".into());
    unsafe {
        let (duration, has_video, _) = probe(src)?;
        if !has_video {
            return Err(VideoError::Unsupported { what: "no video".into() });
        }
        let reader = open_reader(src, true)?;
        let size = reader.GetNativeMediaType(FIRST_VIDEO, 0)?.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (w, h) = ((size >> 32) as u32, size as u32);
        let rgb = MFCreateMediaType()?;
        rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        rgb.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        reader.SetCurrentMediaType(FIRST_VIDEO, None, &rgb).map_err(at("decoding the video"))?;
        let (mut next, mut saved) = (0.0, 0usize);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(VideoError::Cancelled);
            }
            let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
            reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).map_err(at("reading"))?;
            if let Some(sample) = sample.filter(|s| s.GetTotalLength().unwrap_or(0) > 0) {
                let t = ts as f64 / 1e7;
                if t + 1e-4 >= next {
                    next += every * ((t + 1e-4 - next) / every).floor().max(0.0) + every;
                    let rgba = sample_rgba(&sample, w, h)?;
                    let png = crate::image::fast_png(w, h, &rgba);
                    let seconds = t.round() as u64;
                    crate::image::write_new(dir, &format!("{stem} {:02}m{:02}s", seconds / 60, seconds % 60), "png", &png).map_err(|e| VideoError::Io { message: format!("{e:?}") })?;
                    saved += 1;
                    progress((t / duration).clamp(0.0, 1.0));
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                break;
            }
        }
        Ok(saved)
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

    /// Backward pieces with a dissolve and music, a GIF, and a frame every 4 s, from a real video
    /// (JAYE_TEST_VIDEO, `cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn render_more_of_a_real_video() {
        let Some(src) = std::env::var_os("JAYE_TEST_VIDEO").map(std::path::PathBuf::from) else { return };
        let dir = std::env::temp_dir().join(format!("jaye-render2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let options = ConvertOptions { audio_only: false, max_height: Some(360), video_kbps: 1000, audio_kbps: 128 };
        // 2–6 s and 8–12 s, backward, dissolving for 1 s, the video's own sound as "music" too.
        let edits = Render { pieces: vec![[2.0, 6.0], [8.0, 12.0]], reverse: true, dissolve: 1.0, music: Some(Music { path: src.to_string_lossy().into_owned(), volume: 0.5, replace: false }), ..Default::default() };
        let out = dir.join("back.mp4");
        let started = std::time::Instant::now();
        render(&src, &out, &options, &edits, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        let (seconds, video, audio) = probe(&out).unwrap();
        eprintln!("REVERSE {seconds:.2}s in {:?}", started.elapsed());
        assert!(video && audio && (seconds - 7.0).abs() < 0.3, "{seconds}");
        // The first frame of the result is the source's at about 12 s (the second piece, backward).
        let (w, h, first, _) = frame(&out, 0.0).unwrap();
        let (sw, sh, source, _) = frame(&src, 11.9).unwrap();
        let mean = |px: &[u8]| px.chunks(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum::<u64>() / (px.len() as u64 / 4);
        eprintln!("REVERSE first {w}×{h} mean {} vs source {sw}×{sh} mean {}", mean(&first), mean(&source));
        assert!((mean(&first) as i64 - mean(&source) as i64).abs() < 40);
        // A GIF of 1–3 s at 10 fps.
        let gif = dir.join("a.gif");
        render_gif(&src, &gif, &Render { pieces: vec![[1.0, 3.0]], ..Default::default() }, 320, 10, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        let frames = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(std::fs::File::open(&gif).unwrap())).unwrap();
        let frames: Vec<_> = image::AnimationDecoder::into_frames(frames).collect_frames().unwrap();
        eprintln!("GIF {} frames, {}×{}", frames.len(), frames[0].buffer().width(), frames[0].buffer().height());
        assert!((19..=21).contains(&frames.len()) && frames[0].buffer().width() <= 320);
        // Longer than a minute: refused.
        assert!(render_gif(&src, &gif, &Render { speed: 0.25, ..Default::default() }, 320, 10, &mut |_| {}, &AtomicBool::new(false)).is_err());
        // A frame every 4 s of the 16 s video: at 0, 4, 8, 12 (and 16 if it reaches it).
        let n = frames_every(&src, 4.0, &dir, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        let pngs = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "png")).count();
        eprintln!("FRAMES {n} {pngs}");
        assert!((4..=5).contains(&n) && pngs == n);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pieces_reverse_and_dissolves() {
        // Two pieces, the second first, dissolving for 1 s, twice as fast.
        let r = Render { pieces: vec![[4.0, 10.0], [0.0, 3.0]], dissolve: 1.0, speed: 2.0, ..Default::default() };
        let spans = r.spans(20.0);
        assert_eq!(spans, vec![Span { start: 4.0, end: 10.0, backward: false }, Span { start: 0.0, end: 3.0, backward: false }]);
        // Lengths 3 s and 1.5 s at 2×; the dissolve is held to a third of the shorter: 0.5 s.
        assert!((r.overlap(&spans) - 0.5).abs() < 1e-9);
        let (starts, length) = r.layout(&spans);
        assert_eq!(starts, vec![0.0, 2.5]);
        assert!((length - 4.0).abs() < 1e-9);
        // Backward: the pieces in reverse order, each backward.
        let back = Render { reverse: true, ..r.clone() }.spans(20.0);
        assert_eq!(back, vec![Span { start: 0.0, end: 3.0, backward: true }, Span { start: 4.0, end: 10.0, backward: true }]);
        // No pieces: the whole video; pieces past the end are cut to it, empty ones dropped.
        assert_eq!(Render::default().spans(8.0), vec![Span { start: 0.0, end: 8.0, backward: false }]);
        assert_eq!(Render { pieces: vec![[5.0, 50.0], [2.0, 2.0]], ..Default::default() }.spans(8.0), vec![Span { start: 5.0, end: 8.0, backward: false }]);
        // The crossfade: the end of a piece is held, the start of the next mixed with it.
        let mut x = Crossfade::<u8>::new(0.5);
        assert!(matches!(x.place(0, 2, 2.6, 3.0), Place::Hold));
        x.next_held.push((2.6, 1));
        x.next_span();
        assert!(matches!(x.place(1, 2, 0.25, 1.5), Place::Mix(0, a) if (a - 0.5).abs() < 1e-6));
        assert!(matches!(x.place(1, 2, 1.0, 1.5), Place::Write));
        // Music replacing the sound: the video's own goes silent.
        assert_eq!(Render { music: Some(Music { path: "m.mp3".into(), volume: 1.0, replace: true }), ..Default::default() }.gain(), 0.0);
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
