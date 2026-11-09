//! Making a scanned PDF searchable (stage 10) with the OCR that comes with Windows
//! (`Windows.Media.Ocr`, in the languages of the person's Windows): each page without text is drawn,
//! read, and the words it finds go on the page as invisible text, where they are on the picture, so
//! they can be searched, selected and copied.

use crate::pdf_edit::{EditError, deref, flatten, geo, invalid, load, page_id, save, text_width, win_ansi};
use lopdf::{Dictionary, Object, Stream, dictionary};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A word found on a page: `rect` is `[x, y, w, h]` on the shown page, in points.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: [f32; 4],
}

/// The resolution pages are read at.
const DPI: f32 = 300.0;

/// The words Windows reads in a picture (RGBA, `width` × `height`), in its pixels. `Err` when
/// Windows has no OCR for the person's languages.
#[cfg(windows)]
pub fn recognize(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<Vec<Word>>, String> {
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    let e = |e: windows::core::Error| e.message();
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| "no OCR language is installed in Windows".to_owned())?;
    let mut bgra = rgba.to_vec();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let buffer = CryptographicBuffer::CreateFromByteArray(&bgra).map_err(e)?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buffer, BitmapPixelFormat::Bgra8, width as i32, height as i32, BitmapAlphaMode::Premultiplied).map_err(e)?;
    let result = engine.RecognizeAsync(&bitmap).map_err(e)?.join().map_err(e)?;
    let mut lines = vec![];
    for line in result.Lines().map_err(e)? {
        let mut words = vec![];
        for word in line.Words().map_err(e)? {
            let r = word.BoundingRect().map_err(e)?;
            words.push(Word { text: word.Text().map_err(e)?.to_string_lossy(), rect: [r.X, r.Y, r.Width, r.Height] });
        }
        lines.push(words);
    }
    Ok(lines)
}

#[cfg(not(windows))]
pub fn recognize(_: u32, _: u32, _: &[u8]) -> Result<Vec<Vec<Word>>, String> {
    Err("OCR needs Windows".into())
}

/// Puts `lines` of words on page `page` as invisible text (Helvetica, stretched to each word's box).
pub fn add_text(bytes: &[u8], page: usize, lines: &[Vec<Word>]) -> Result<Vec<u8>, EditError> {
    let mut doc = load(bytes)?;
    let pages = flatten(&mut doc)?;
    let id = page_id(&pages, page)?;
    let g = geo(&doc, id);
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
    let key = add_font(&mut doc, id, font)?;
    let mut text = String::from("Q q BT 3 Tr\n");
    for word in lines.iter().flatten() {
        let [x, y, w, h] = word.rect;
        let bytes = win_ansi(&word.text);
        if bytes.is_empty() || w <= 0.0 || h <= 0.0 {
            continue;
        }
        // The shown page's right and up, in the page's own space (it may be turned).
        let baseline = y + h * 0.8;
        let o = g.to_user([x, baseline]);
        let right = g.to_user([x + 1.0, baseline]);
        let up = g.to_user([x, baseline - 1.0]);
        let natural = text_width(&word.text, h).max(0.1);
        let stretch = (w / natural * 100.0).clamp(1.0, 1000.0);
        let literal: String = bytes.iter().map(|&b| if b == b'(' || b == b')' || b == b'\\' { format!("\\{}", b as char) } else if b.is_ascii_graphic() || b == b' ' { (b as char).to_string() } else { format!("\\{b:03o}") }).collect();
        text.push_str(&format!("{} {} {} {} {} {} Tm /{} {h:.2} Tf {stretch:.2} Tz ({literal}) Tj\n", right[0] - o[0], right[1] - o[1], up[0] - o[0], up[1] - o[1], o[0], o[1], String::from_utf8_lossy(&key)));
    }
    text.push_str("ET Q\n");
    // The page's own content inside q … Q, so the text starts from a clean state.
    let before = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let mut after = Stream::new(Dictionary::new(), text.into_bytes());
    let _ = after.compress();
    let after = doc.add_object(after);
    let page_dict = doc.get_dictionary(id).map_err(invalid)?;
    let mut contents: Vec<Object> = match page_dict.get(b"Contents").ok().map(|o| (o.clone(), deref(&doc, o).clone())) {
        Some((_, Object::Array(a))) => a,
        Some((r @ Object::Reference(_), _)) => vec![r],
        _ => vec![],
    };
    contents.insert(0, Object::Reference(before));
    contents.push(Object::Reference(after));
    doc.get_dictionary_mut(id).map_err(invalid)?.set("Contents", Object::Array(contents));
    save(&mut doc)
}

fn add_font(doc: &mut lopdf::Document, page: lopdf::ObjectId, font: lopdf::ObjectId) -> Result<Vec<u8>, EditError> {
    let res = match doc.get_dictionary(page).map_err(invalid)?.get(b"Resources").ok().cloned() {
        Some(Object::Reference(r)) => doc.get_dictionary(r).map_err(invalid)?.clone(),
        Some(Object::Dictionary(d)) => d,
        _ => Dictionary::new(),
    };
    let mut fonts = match res.get(b"Font").ok().map(|o| deref(doc, o)) {
        Some(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    let key = (1..).map(|i| format!("JEocr{i}").into_bytes()).find(|k| !fonts.has(k)).unwrap();
    fonts.set(key.clone(), Object::Reference(font));
    let mut res = res;
    res.set("Font", Object::Dictionary(fonts));
    doc.get_dictionary_mut(page).map_err(invalid)?.set("Resources", Object::Dictionary(res));
    Ok(key)
}

/// Every page with no text read and given its words. `progress(done, pages)` after each page.
/// Returns the new file and how many pages got text.
pub fn make_searchable(bytes: &[u8], progress: &mut dyn FnMut(usize, usize), cancel: &AtomicBool) -> Result<(Vec<u8>, usize), EditError> {
    let shown = crate::pdf::PdfDocument::from_bytes(Arc::new(bytes.to_vec()), "").map_err(|e| invalid(format!("{e:?}")))?;
    let pages = shown.info.pages.clone();
    let mut out = bytes.to_vec();
    let mut done = 0;
    for (page, [w, _]) in pages.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(invalid("cancelled"));
        }
        let has_text = crate::pdf_content::page_content(&out, page).is_ok_and(|c| !c.lines.is_empty());
        if !has_text {
            let width = (w / 72.0 * DPI).clamp(200.0, 9000.0) as u32;
            let bitmap = shown.render(page, width, false).recv().ok().flatten().ok_or_else(|| invalid("the page couldn't be drawn"))?;
            let lines = recognize(bitmap.width, bitmap.height, &bitmap.rgba).map_err(invalid)?;
            if lines.iter().any(|l| !l.is_empty()) {
                // From pixels to points.
                let k = w / bitmap.width as f32;
                let lines: Vec<Vec<Word>> = lines.into_iter().map(|l| l.into_iter().map(|wd| Word { text: wd.text, rect: wd.rect.map(|v| v * k) }).collect()).collect();
                out = add_text(&out, page, &lines)?;
                done += 1;
            }
        }
        progress(page + 1, pages.len());
    }
    Ok((out, done))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page that's only a picture of the words "HELLO WORLD" (drawn here as text, then drawn
    /// to pixels, then the text taken away).
    #[test]
    #[cfg(windows)]
    fn a_picture_of_words_becomes_searchable() {
        let mut doc = lopdf::Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let helv = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let content = doc.add_object(Stream::new(Dictionary::new(), b"BT /F1 36 Tf 72 700 Td (HELLO WORLD) Tj ET".to_vec()));
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => content, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()], "Resources" => dictionary! { "Font" => dictionary! { "F1" => helv } } });
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1 }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        let text_pdf = save(&mut doc).unwrap();
        let drawn = crate::pdf::PdfDocument::from_bytes(Arc::new(text_pdf), "").unwrap().render(0, 1275, false).recv().unwrap().unwrap();
        let picture = image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(drawn.width, drawn.height, drawn.rgba).unwrap());
        let scanned = crate::pdf_content::images_to_pdf(&[(picture, None)]).unwrap();
        assert!(crate::pdf_content::page_content(&scanned, 0).unwrap().lines.is_empty());
        let mut calls = 0;
        match make_searchable(&scanned, &mut |_, _| calls += 1, &AtomicBool::new(false)) {
            Ok((out, done)) => {
                assert_eq!((done, calls), (1, 1));
                let lines = crate::pdf_content::page_content(&out, 0).unwrap().lines;
                let all: String = lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join(" ");
                assert!(all.contains("HELLO") && all.contains("WORLD"), "{all:?}");
                // Where the words are on the picture: near the top.
                assert!(lines[0].rect[1] < 300.0, "{:?}", lines[0].rect);
                // Again: nothing to do, the page has text now.
                assert_eq!(make_searchable(&out, &mut |_, _| {}, &AtomicBool::new(false)).unwrap().1, 0);
            }
            // A Windows without an OCR language (some CI images): nothing to check.
            Err(EditError::Invalid { message }) if message.contains("OCR") => eprintln!("skipped: {message}"),
            Err(e) => panic!("{e:?}"),
        }
    }
}
