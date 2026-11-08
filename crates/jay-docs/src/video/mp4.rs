//! Cutting and joining MP4/MOV without re-encoding (stream copy, spike S4): the sample tables of
//! each track are read from `moov`, the samples to keep are chosen, and a new file is written with
//! `moov` first (it plays while loading) and only those samples' bytes copied. Instant, lossless.
//!
//! A cut starts at the key frame at or before the start asked (the only place a video can start
//! without decoding); the result says where it really starts. Fragmented MP4 (`moof`) isn't read.

use super::VideoError;
use mp4_atom::{Atom, Co64, Encode, Ctts, CttsEntry, ElstEntry, FourCC, Ftyp, Header, Moov, ReadAtom, ReadFrom, Stsc, StscEntry, Stss, Stsz, StszSamples, Stts, SttsEntry, Trak};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// A file with more samples isn't read (a 10-hour 60 fps video has ~2 M).
const MAX_SAMPLES: usize = 20_000_000;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    offset: u64,
    size: u32,
    dts: u64,
    duration: u32,
    /// Composition offset (ctts).
    cts: i64,
    sync: bool,
    /// Sample description index (1-based), from stsc.
    desc: u32,
    /// Which input file (for joins).
    source: usize,
}

#[derive(Debug, Clone)]
struct Track {
    trak: Trak,
    timescale: u32,
    video: bool,
    samples: Vec<Sample>,
}

/// An open MP4: its atoms and the sample tables of every track.
#[derive(Debug, Clone)]
pub struct Movie {
    ftyp: Option<Ftyp>,
    moov: Moov,
    tracks: Vec<Track>,
}

impl Movie {
    pub fn open(path: &Path) -> Result<Self, VideoError> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let (mut ftyp, mut moov) = (None, None);
        loop {
            let pos = file.stream_position()?;
            if pos >= len {
                break;
            }
            let header = match Header::read_from(&mut file) {
                Ok(h) => h,
                Err(_) if moov.is_some() => break, // trailing junk after a complete file
                Err(e) => return Err(VideoError::Invalid { message: e.to_string() }),
            };
            let body_start = file.stream_position()?;
            let size = header.size.map(|s| s as u64).unwrap_or(len - body_start);
            if body_start + size > len {
                return Err(VideoError::Invalid { message: format!("atom {} goes past the end of the file", header.kind) });
            }
            match header.kind {
                k if k == Ftyp::KIND => ftyp = Some(Ftyp::read_atom(&header, &mut file).map_err(invalid)?),
                k if k == Moov::KIND => {
                    if size > 512 << 20 {
                        return Err(VideoError::TooBig);
                    }
                    moov = Some(Moov::read_atom(&header, &mut file).map_err(invalid)?);
                }
                k if k == FourCC::new(b"moof") => return Err(VideoError::Unsupported { what: "fragmented MP4".into() }),
                _ => {}
            }
            file.seek(SeekFrom::Start(body_start + size))?;
        }
        let moov = moov.ok_or_else(|| VideoError::Invalid { message: "no moov (an incomplete recording?)".into() })?;
        let mut tracks = vec![];
        let mut total = 0;
        for trak in &moov.trak {
            let samples = samples_of(trak, len)?;
            total += samples.len();
            if total > MAX_SAMPLES {
                return Err(VideoError::TooBig);
            }
            tracks.push(Track { timescale: trak.mdia.mdhd.timescale.max(1), video: trak.mdia.hdlr.handler == FourCC::new(b"vide"), samples, trak: trak.clone() });
        }
        if tracks.iter().all(|t| t.samples.is_empty()) {
            return Err(VideoError::Invalid { message: "no samples".into() });
        }
        Ok(Self { ftyp, moov, tracks })
    }

    /// The longest track, in seconds.
    pub fn duration(&self) -> f64 {
        self.tracks.iter().map(|t| t.samples.last().map(|s| (s.dts + s.duration as u64) as f64 / t.timescale as f64).unwrap_or(0.0)).fold(0.0, f64::max)
    }

    /// Key frame times of the first video track, in seconds (where a cut can start).
    pub fn key_frames(&self) -> Vec<f64> {
        self.tracks.iter().find(|t| t.video).map(|t| t.samples.iter().filter(|s| s.sync).map(|s| s.dts as f64 / t.timescale as f64).collect()).unwrap_or_default()
    }

    /// The first video track's average frame rate, for stepping frame by frame.
    pub fn frame_rate(&self) -> Option<f64> {
        let t = self.tracks.iter().find(|t| t.video)?;
        let last = t.samples.last()?;
        let seconds = (last.dts + last.duration as u64) as f64 / t.timescale as f64;
        (seconds > 0.0).then(|| t.samples.len() as f64 / seconds)
    }

    /// The samples of `[start, end)` seconds, from the key frame at or before `start`, with times
    /// from 0; and where that really starts, and how long it is.
    fn piece(&self, start: f64, end: f64) -> (Vec<Track>, f64, f64) {
        let reference = self.tracks.iter().position(|t| t.video).unwrap_or(0);
        let r = &self.tracks[reference];
        let start_sample = r.samples.iter().rposition(|s| s.sync && (s.dts as f64) <= start * r.timescale as f64).unwrap_or(0);
        let t0 = r.samples.get(start_sample).map(|s| s.dts as f64 / r.timescale as f64).unwrap_or(0.0);
        let end = end.max(t0 + 0.001);
        let mut tracks = vec![];
        for t in &self.tracks {
            let ts = t.timescale as f64;
            let (from, to) = ((t0 * ts).round() as u64, (end * ts).round() as u64);
            let kept: Vec<Sample> = t.samples.iter().filter(|s| s.dts >= from && s.dts < to).map(|s| Sample { dts: s.dts - from, ..*s }).collect();
            tracks.push(Track { samples: kept, ..t.clone() });
        }
        let length = tracks.iter().map(|t| t.samples.last().map(|s| (s.dts + s.duration as u64) as f64 / t.timescale as f64).unwrap_or(0.0)).fold(0.0, f64::max);
        (tracks, t0, length)
    }

    /// Writes `[start, end)` seconds to `dst`, from the key frame at or before `start`. Returns the
    /// real start and end.
    pub fn cut(&self, src: &Path, start: f64, end: f64, dst: &Path, cancel: &AtomicBool) -> Result<(f64, f64), VideoError> {
        let (tracks, t0, length) = self.piece(start, end);
        write(self, &tracks, &[src], dst, cancel)?;
        Ok((t0, t0 + length))
    }

    /// Several pieces of this movie, in the order given, one after another in `dst` (the razor:
    /// split, drop the pieces in between, reorder). Each starts at the key frame at or before its
    /// start, as a cut does; returns where each really starts and ends.
    pub fn splice(&self, src: &Path, pieces: &[(f64, f64)], dst: &Path, cancel: &AtomicBool) -> Result<Vec<(f64, f64)>, VideoError> {
        if pieces.is_empty() || pieces.iter().any(|(a, b)| !a.is_finite() || !b.is_finite() || b <= a) {
            return Err(VideoError::Invalid { message: "no pieces to keep".into() });
        }
        let mut out: Vec<Track> = self.tracks.iter().map(|t| Track { samples: vec![], ..t.clone() }).collect();
        let mut offset = 0.0; // seconds where the next piece starts
        let mut real = vec![];
        for &(start, end) in pieces {
            let (tracks, t0, length) = self.piece(start, end);
            for (o, t) in out.iter_mut().zip(&tracks) {
                let at = (offset * t.timescale as f64).round() as u64;
                // The previous piece's last sample lasts until this one (tracks rarely end together).
                if let Some(last) = o.samples.last_mut() {
                    last.duration = at.saturating_sub(last.dts).min(u32::MAX as u64) as u32;
                }
                o.samples.extend(t.samples.iter().map(|s| Sample { dts: s.dts + at, ..*s }));
            }
            offset += length;
            real.push((t0, t0 + length));
        }
        write(self, &out, &[src], dst, cancel)?;
        Ok(real)
    }
}

/// Joins `movies` (in order) into `dst`. They must have the same tracks with the same codecs and
/// settings (sample descriptions), as parts of one recording or cuts of one file do.
pub fn join(movies: &[(&Path, &Movie)], dst: &Path, cancel: &AtomicBool) -> Result<(), VideoError> {
    let (_, first) = movies.first().ok_or_else(|| VideoError::Invalid { message: "nothing to join".into() })?;
    for (path, m) in &movies[1..] {
        let same = m.tracks.len() == first.tracks.len()
            && m.tracks.iter().zip(&first.tracks).all(|(a, b)| a.timescale == b.timescale && encoded(&a.trak.mdia.minf.stbl.stsd) == encoded(&b.trak.mdia.minf.stbl.stsd));
        if !same {
            return Err(VideoError::Mismatch { file: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() });
        }
    }
    let mut tracks: Vec<Track> = first.tracks.iter().map(|t| Track { samples: vec![], ..t.clone() }).collect();
    let mut offset = 0.0; // seconds where the next file starts
    for (source, (_, m)) in movies.iter().enumerate() {
        for (out, t) in tracks.iter_mut().zip(&m.tracks) {
            let start = (offset * t.timescale as f64).round() as u64;
            // Stretch the previous part's last sample to meet this part (tracks rarely end together).
            if let Some(last) = out.samples.last_mut() {
                last.duration = start.saturating_sub(last.dts).min(u32::MAX as u64) as u32;
            }
            out.samples.extend(t.samples.iter().map(|s| Sample { dts: s.dts + start, source, ..*s }));
        }
        offset += m.duration();
    }
    let sources: Vec<&Path> = movies.iter().map(|(p, _)| *p).collect();
    write(first, &tracks, &sources, dst, cancel)
}

fn invalid(e: mp4_atom::Error) -> VideoError {
    VideoError::Invalid { message: e.to_string() }
}

fn encoded<A: Encode>(atom: &A) -> Vec<u8> {
    let mut v = Vec::new();
    let _ = atom.encode(&mut v);
    v
}

/// Every sample of a track, from its sample tables.
fn samples_of(trak: &Trak, file_len: u64) -> Result<Vec<Sample>, VideoError> {
    let stbl = &trak.mdia.minf.stbl;
    let sizes: Vec<u32> = match &stbl.stsz.samples {
        StszSamples::Identical { count, size } => {
            if *count as usize > MAX_SAMPLES {
                return Err(VideoError::TooBig);
            }
            vec![*size; *count as usize]
        }
        StszSamples::Different { sizes } => sizes.clone(),
    };
    let chunks: Vec<u64> = match (&stbl.co64, &stbl.stco) {
        (Some(c), _) => c.entries.clone(),
        (None, Some(c)) => c.entries.iter().map(|&o| o as u64).collect(),
        (None, None) => vec![],
    };
    let mut samples = Vec::with_capacity(sizes.len());
    // stsc: runs of chunks with the same number of samples.
    let mut n = 0usize;
    for (ci, &chunk_offset) in chunks.iter().enumerate() {
        let chunk = ci as u32 + 1;
        let Some(entry) = stbl.stsc.entries.iter().rev().find(|e| e.first_chunk <= chunk) else { break };
        let mut offset = chunk_offset;
        for _ in 0..entry.samples_per_chunk {
            let Some(&size) = sizes.get(n) else { break };
            if offset + size as u64 > file_len {
                return Err(VideoError::Invalid { message: "a sample goes past the end of the file".into() });
            }
            samples.push(Sample { offset, size, dts: 0, duration: 0, cts: 0, sync: stbl.stss.is_none(), desc: entry.sample_description_index, source: 0 });
            offset += size as u64;
            n += 1;
        }
    }
    let mut i = 0usize;
    let mut dts = 0u64;
    for e in &stbl.stts.entries {
        for _ in 0..e.sample_count {
            let Some(s) = samples.get_mut(i) else { break };
            s.dts = dts;
            s.duration = e.sample_delta;
            dts += e.sample_delta as u64;
            i += 1;
        }
    }
    if let Some(ctts) = &stbl.ctts {
        let mut i = 0usize;
        for e in &ctts.entries {
            for _ in 0..e.sample_count {
                let Some(s) = samples.get_mut(i) else { break };
                s.cts = e.sample_offset;
                i += 1;
            }
        }
    }
    if let Some(stss) = &stbl.stss {
        for &k in &stss.entries {
            if let Some(s) = samples.get_mut((k as usize).wrapping_sub(1)) {
                s.sync = true;
            }
        }
    }
    Ok(samples)
}

/// Writes ftyp, moov (with new sample tables) and one mdat with the samples interleaved by time.
fn write(template: &Movie, tracks: &[Track], sources: &[&Path], dst: &Path, cancel: &AtomicBool) -> Result<(), VideoError> {
    // The order samples go in mdat: by time, across tracks.
    let mut order: Vec<(usize, usize)> = tracks.iter().enumerate().flat_map(|(t, tr)| (0..tr.samples.len()).map(move |i| (t, i))).collect();
    order.sort_by(|a, b| {
        let ta = tracks[a.0].samples[a.1].dts as f64 / tracks[a.0].timescale as f64;
        let tb = tracks[b.0].samples[b.1].dts as f64 / tracks[b.0].timescale as f64;
        ta.total_cmp(&tb).then(a.0.cmp(&b.0))
    });
    let mut moov = template.moov.clone();
    let movie_scale = moov.mvhd.timescale.max(1) as u64;
    let mut max_duration = 0u64;
    moov.trak = tracks
        .iter()
        .map(|t| {
            let mut trak = t.trak.clone();
            let stbl = &mut trak.mdia.minf.stbl;
            stbl.stts = Stts { entries: runs(t.samples.iter().map(|s| s.duration)).into_iter().map(|(sample_count, sample_delta)| SttsEntry { sample_count, sample_delta }).collect() };
            stbl.ctts = (t.samples.iter().any(|s| s.cts != 0)).then(|| Ctts { entries: runs(t.samples.iter().map(|s| s.cts)).into_iter().map(|(sample_count, sample_offset)| CttsEntry { sample_count, sample_offset }).collect() });
            stbl.stss = stbl.stss.is_some().then(|| Stss { entries: t.samples.iter().enumerate().filter(|(_, s)| s.sync).map(|(i, _)| i as u32 + 1).collect() });
            // One chunk per sample; a new stsc entry only where the description changes.
            let mut stsc = vec![];
            let mut last_desc = 0;
            for (i, s) in t.samples.iter().enumerate() {
                if s.desc != last_desc {
                    stsc.push(StscEntry { first_chunk: i as u32 + 1, samples_per_chunk: 1, sample_description_index: s.desc });
                    last_desc = s.desc;
                }
            }
            stbl.stsc = Stsc { entries: stsc };
            stbl.stsz = Stsz { samples: StszSamples::Different { sizes: t.samples.iter().map(|s| s.size).collect() } };
            stbl.stco = None;
            stbl.co64 = Some(Co64 { entries: vec![0; t.samples.len()] });
            stbl.sbgp.clear();
            stbl.sgpd.clear();
            stbl.subs.clear();
            stbl.saiz.clear();
            stbl.saio.clear();
            stbl.cslg = None;
            let media = t.samples.iter().map(|s| s.duration as u64).sum::<u64>();
            trak.mdia.mdhd.duration = media;
            let movie = media * movie_scale / t.timescale as u64;
            trak.tkhd.duration = movie;
            max_duration = max_duration.max(movie);
            // Keep a single edit's start offset (B-frame delay); drop anything fancier.
            trak.edts = trak.edts.take().and_then(|mut edts| {
                let elst = edts.elst.as_mut()?;
                let media_time = match elst.entries.as_slice() {
                    [ElstEntry { media_time: Some(m), .. }] => *m,
                    _ => return None,
                };
                elst.entries = vec![ElstEntry { segment_duration: movie, media_time: Some(media_time), media_rate: elst.entries[0].media_rate.clone() }];
                Some(edts)
            });
            trak
        })
        .collect();
    moov.mvhd.duration = max_duration;

    let ftyp = template.ftyp.as_ref().map(encoded).unwrap_or_default();
    let moov_len = encoded(&moov).len() as u64;
    // mdat with a 64-bit size: 16-byte header.
    let mut offset = ftyp.len() as u64 + moov_len + 16;
    let mut offsets: Vec<Vec<u64>> = tracks.iter().map(|t| vec![0; t.samples.len()]).collect();
    for &(t, i) in &order {
        offsets[t][i] = offset;
        offset += tracks[t].samples[i].size as u64;
    }
    for (trak, offs) in moov.trak.iter_mut().zip(offsets) {
        trak.mdia.minf.stbl.co64 = Some(Co64 { entries: offs });
    }
    let moov = encoded(&moov);
    debug_assert_eq!(moov.len() as u64, moov_len);
    let data_len = offset - (ftyp.len() as u64 + moov_len + 16);

    let tmp = dst.with_extension("jaye-tmp");
    let result = (|| {
        let mut out = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
        out.write_all(&ftyp)?;
        out.write_all(&moov)?;
        out.write_all(&1u32.to_be_bytes())?;
        out.write_all(b"mdat")?;
        out.write_all(&(data_len + 16).to_be_bytes())?;
        let mut files: Vec<File> = sources.iter().map(File::open).collect::<Result<_, _>>()?;
        let mut buf = vec![];
        for (n, &(t, i)) in order.iter().enumerate() {
            if n % 256 == 0 && cancel.load(Ordering::Relaxed) {
                return Err(VideoError::Cancelled);
            }
            let s = tracks[t].samples[i];
            let f = &mut files[s.source];
            f.seek(SeekFrom::Start(s.offset))?;
            buf.resize(s.size as usize, 0);
            f.read_exact(&mut buf)?;
            out.write_all(&buf)?;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        Ok(())
    })();
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

/// Run-length encoding: (count, value).
fn runs<T: PartialEq + Copy>(values: impl Iterator<Item = T>) -> Vec<(u32, T)> {
    let mut out: Vec<(u32, T)> = vec![];
    for v in values {
        match out.last_mut() {
            Some((n, last)) if *last == v => *n += 1,
            _ => out.push((1, v)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mp4_atom::{Dinf, Hdlr, Mdhd, Mdia, Minf, Mvhd, Stbl, Stco, Tkhd};

    /// An MP4 with a "video" track (fake bytes: sample i is `i` repeated) at 10 samples per second,
    /// a key frame every 10, and an "audio" track at 20 per second. Nothing decodes it; the
    /// structure is what's tested.
    fn sample_file(dir: &Path, name: &str, seconds: u32) -> std::path::PathBuf {
        let mut mdat = vec![];
        let mut video = vec![];
        let mut audio = vec![];
        for i in 0..seconds * 20 {
            if i % 2 == 0 {
                let v = i / 2;
                video.push((mdat.len() as u32, 100 + v % 7));
                mdat.extend(std::iter::repeat_n((v % 251) as u8, (100 + v % 7) as usize));
            }
            audio.push((mdat.len() as u32, 20));
            mdat.extend([0xA5u8; 20]);
        }
        let trak = |id: u32, handler: &[u8; 4], scale: u32, delta: u32, samples: &[(u32, u32)], keys: Option<Vec<u32>>, base: u32| Trak {
            tkhd: Tkhd { track_id: id, duration: (samples.len() as u64 * delta as u64) * 1000 / scale as u64, ..Default::default() },
            mdia: Mdia {
                mdhd: Mdhd { timescale: scale, duration: samples.len() as u64 * delta as u64, language: "und".into(), ..Default::default() },
                hdlr: Hdlr { handler: FourCC::new(handler), name: String::new() },
                minf: Minf {
                    dinf: Dinf::default(),
                    stbl: Stbl {
                        stts: Stts { entries: vec![SttsEntry { sample_count: samples.len() as u32, sample_delta: delta }] },
                        stss: keys.map(|entries| Stss { entries }),
                        stsc: Stsc { entries: vec![StscEntry { first_chunk: 1, samples_per_chunk: 1, sample_description_index: 1 }] },
                        stsz: Stsz { samples: StszSamples::Different { sizes: samples.iter().map(|s| s.1).collect() } },
                        stco: Some(Stco { entries: samples.iter().map(|s| s.0 + base).collect() }),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
            ..Default::default()
        };
        let build = |base: u32| Moov {
            mvhd: Mvhd { timescale: 1000, duration: seconds as u64 * 1000, next_track_id: 3, ..Default::default() },
            trak: vec![
                trak(1, b"vide", 10_000, 1000, &video, Some((0..video.len() as u32).step_by(10).map(|i| i + 1).collect()), base),
                trak(2, b"soun", 48_000, 2400, &audio, None, base),
            ],
            ..Default::default()
        };
        let ftyp = encoded(&Ftyp { major_brand: FourCC::new(b"isom"), minor_version: 512, compatible_brands: vec![FourCC::new(b"isom")] });
        let moov_len = encoded(&build(0)).len() as u32;
        let base = ftyp.len() as u32 + moov_len + 8;
        let mut out = ftyp;
        out.extend(encoded(&build(base)));
        out.extend((mdat.len() as u32 + 8).to_be_bytes());
        out.extend(b"mdat");
        out.extend(mdat);
        let p = dir.join(name);
        std::fs::write(&p, out).unwrap();
        p
    }

    fn dir() -> std::path::PathBuf {
        use std::sync::atomic::AtomicU32;
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!("jay-docs-mp4-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The bytes of video sample `i` of a movie.
    fn video_bytes(path: &Path, m: &Movie, i: usize) -> Vec<u8> {
        let s = m.tracks[0].samples[i];
        let mut f = File::open(path).unwrap();
        f.seek(SeekFrom::Start(s.offset)).unwrap();
        let mut b = vec![0; s.size as usize];
        f.read_exact(&mut b).unwrap();
        b
    }

    #[test]
    fn reads_the_tables() {
        let d = dir();
        let p = sample_file(&d, "a.mp4", 6);
        let m = Movie::open(&p).unwrap();
        assert_eq!(m.tracks[0].samples.len(), 60);
        assert_eq!(m.tracks[1].samples.len(), 120);
        assert!((m.duration() - 6.0).abs() < 1e-9);
        assert_eq!(m.key_frames(), vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(video_bytes(&p, &m, 13), vec![13; 106]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn cut_starts_on_a_key_frame_and_copies_the_right_bytes() {
        let d = dir();
        let p = sample_file(&d, "a.mp4", 6);
        let m = Movie::open(&p).unwrap();
        let out = d.join("cut.mp4");
        // 2.5 s → back to the key frame at 2.0; until 4.0.
        let (start, end) = m.cut(&p, 2.5, 4.0, &out, &AtomicBool::new(false)).unwrap();
        assert_eq!((start, end), (2.0, 4.0));
        let c = Movie::open(&out).unwrap();
        assert_eq!(c.tracks[0].samples.len(), 20);
        assert_eq!(c.tracks[1].samples.len(), 40);
        assert!(c.tracks[0].samples[0].sync && c.tracks[0].samples[0].dts == 0);
        assert_eq!(c.key_frames(), vec![0.0, 1.0]);
        for i in 0..20 {
            assert_eq!(video_bytes(&out, &c, i), video_bytes(&p, &m, 20 + i), "sample {i}");
        }
        // moov before mdat, so it plays while loading.
        let bytes = std::fs::read(&out).unwrap();
        let moov_at = bytes.windows(4).position(|w| w == b"moov").unwrap();
        let mdat_at = bytes.windows(4).position(|w| w == b"mdat").unwrap();
        assert!(moov_at < mdat_at);
        assert!((c.duration() - 2.0).abs() < 1e-9);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn join_puts_parts_end_to_end() {
        let d = dir();
        let a = sample_file(&d, "a.mp4", 3);
        let b = sample_file(&d, "b.mp4", 2);
        let (ma, mb) = (Movie::open(&a).unwrap(), Movie::open(&b).unwrap());
        let out = d.join("joined.mp4");
        join(&[(&a, &ma), (&b, &mb)], &out, &AtomicBool::new(false)).unwrap();
        let j = Movie::open(&out).unwrap();
        assert_eq!(j.tracks[0].samples.len(), 50);
        assert!((j.duration() - 5.0).abs() < 1e-9);
        assert_eq!(j.key_frames(), vec![0.0, 1.0, 2.0, 3.0, 4.0]);
        assert_eq!(video_bytes(&out, &j, 30), video_bytes(&b, &mb, 0));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn splice_keeps_pieces_in_the_order_given() {
        let d = dir();
        let p = sample_file(&d, "a.mp4", 6);
        let m = Movie::open(&p).unwrap();
        assert!((m.frame_rate().unwrap() - 10.0).abs() < 1e-9);
        let out = d.join("spliced.mp4");
        // 4–5 s first, then 1–2 s (the middle dropped); 1.5 starts back at its key frame, 1.0.
        let real = m.splice(&p, &[(4.0, 5.0), (1.5, 2.0)], &out, &AtomicBool::new(false)).unwrap();
        assert_eq!(real, vec![(4.0, 5.0), (1.0, 2.0)]);
        let s = Movie::open(&out).unwrap();
        assert_eq!(s.tracks[0].samples.len(), 20);
        assert!((s.duration() - 2.0).abs() < 1e-9);
        assert_eq!(s.key_frames(), vec![0.0, 1.0]);
        assert_eq!(video_bytes(&out, &s, 0), video_bytes(&p, &m, 40), "starts with 4.0 s");
        assert_eq!(video_bytes(&out, &s, 10), video_bytes(&p, &m, 10), "then 1.0 s");
        assert!(m.splice(&p, &[], &out, &AtomicBool::new(false)).is_err());
        assert!(m.splice(&p, &[(2.0, 1.0)], &out, &AtomicBool::new(false)).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn broken_files_and_cancel() {
        let d = dir();
        let p = sample_file(&d, "a.mp4", 3);
        let bytes = std::fs::read(&p).unwrap();
        // Cut before the end of mdat: the tables point past the end.
        std::fs::write(d.join("short.mp4"), &bytes[..bytes.len() - 500]).unwrap();
        assert!(matches!(Movie::open(&d.join("short.mp4")), Err(VideoError::Invalid { .. })));
        std::fs::write(d.join("junk.mp4"), b"\0\0\0\x08junkxxxxxxxx").unwrap();
        assert!(Movie::open(&d.join("junk.mp4")).is_err());
        let m = Movie::open(&p).unwrap();
        let out = d.join("c.mp4");
        assert!(matches!(m.cut(&p, 0.0, 3.0, &out, &AtomicBool::new(true)), Err(VideoError::Cancelled)));
        assert!(!out.exists() && !out.with_extension("jaye-tmp").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
