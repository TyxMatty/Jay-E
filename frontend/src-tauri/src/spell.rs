//! Spelling in the document's own language (stage 10) with the spell checker that comes with
//! Windows (ISpellCheckerFactory, the one Word and Edge use), whatever language the window is in:
//! the page sends the words it shows and underlines the ones that come back.

/// The language Windows can check closest to `tag` ("pt-BR", or the first "pt-…" if only that).
#[cfg(windows)]
fn checker(tag: &str) -> Result<Option<(windows::Win32::Globalization::ISpellChecker, String)>, String> {
    use windows::Win32::Globalization::{ISpellCheckerFactory, SpellCheckerFactory};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::core::HSTRING;
    let e = |e: windows::core::Error| e.message();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: ISpellCheckerFactory = CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).map_err(e)?;
        let mut chosen = None;
        if factory.IsSupported(&HSTRING::from(tag)).map_err(e)?.as_bool() {
            chosen = Some(tag.to_owned());
        } else {
            let primary = tag.split('-').next().unwrap_or(tag).to_ascii_lowercase();
            let all = strings(&factory.SupportedLanguages().map_err(e)?);
            chosen = all.into_iter().find(|l| l.split('-').next().is_some_and(|p| p.eq_ignore_ascii_case(&primary))).or(chosen);
        }
        let Some(lang) = chosen else { return Ok(None) };
        Ok(Some((factory.CreateSpellChecker(&HSTRING::from(lang.as_str())).map_err(e)?, lang)))
    }
}

#[cfg(windows)]
fn strings(list: &windows::Win32::System::Com::IEnumString) -> Vec<String> {
    let mut out = vec![];
    loop {
        let mut item = [windows::core::PWSTR::null()];
        let mut fetched = 0;
        let hr = unsafe { list.Next(&mut item, Some(&mut fetched)) };
        if hr.is_err() || fetched == 0 || out.len() > 500 {
            break;
        }
        unsafe {
            out.push(item[0].to_string().unwrap_or_default());
            windows::Win32::System::Com::CoTaskMemFree(Some(item[0].0 as _));
        }
    }
    out
}

#[derive(serde::Serialize)]
pub struct Checked {
    /// The language checked in, as Windows names it.
    lang: String,
    /// The words that are misspelled.
    wrong: Vec<String>,
}

/// Which of `words` are misspelled in `lang`. `None` when Windows has no dictionary for it.
#[tauri::command]
pub async fn spell_check(lang: String, words: Vec<String>) -> Result<Option<Checked>, String> {
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || {
            let Some((checker, lang)) = checker(&lang)? else { return Ok(None) };
            let mut wrong = vec![];
            for word in words.iter().take(20_000) {
                let errors = unsafe { checker.Check(&windows::core::HSTRING::from(word.as_str())) }.map_err(|e| e.message())?;
                let mut first = None;
                // S_OK with an error means a mistake; S_FALSE, none.
                if unsafe { errors.Next(&mut first) }.0 == 0 && first.is_some() {
                    wrong.push(word.clone());
                }
            }
            Ok(Some(Checked { lang, wrong }))
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (lang, words);
        Ok(None)
    }
}

/// Up to six spellings Windows suggests for `word`.
#[tauri::command]
pub async fn spell_suggest(lang: String, word: String) -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || {
            let Some((checker, _)) = checker(&lang)? else { return Ok(vec![]) };
            let list = unsafe { checker.Suggest(&windows::core::HSTRING::from(word.as_str())) }.map_err(|e| e.message())?;
            Ok(strings(&list).into_iter().take(6).collect())
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (lang, word);
        Ok(vec![])
    }
}

/// Adds `word` to the person's Windows dictionary for `lang` (the one Word and Edge use too).
#[tauri::command]
pub async fn spell_add(lang: String, word: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || {
            let Some((checker, _)) = checker(&lang)? else { return Ok(()) };
            unsafe { checker.Add(&windows::core::HSTRING::from(word.as_str())) }.map_err(|e| e.message())
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (lang, word);
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use tauri::async_runtime::block_on;

    /// In whichever of these Windows has a dictionary for (a test PC may have only one).
    #[test]
    fn words_are_checked_in_the_language_asked() {
        let mut checked_any = false;
        for (lang, words, wrong, fix) in [("en-US", ["house", "hosue", "Paris"], "hosue", "house"), ("pt-BR", ["casa", "ortgrafia", "Brasil"], "ortgrafia", "ortografia")] {
            let Some(c) = block_on(super::spell_check(lang.into(), words.map(String::from).to_vec())).unwrap() else { continue };
            checked_any = true;
            assert_eq!(c.wrong, [wrong], "{lang}");
            let suggestions = block_on(super::spell_suggest(lang.into(), wrong.into())).unwrap();
            assert!(suggestions.iter().any(|s| s == fix), "{lang}: {suggestions:?}");
        }
        if !checked_any {
            eprintln!("skipped: no English or Portuguese dictionary in this Windows");
        }
        assert!(block_on(super::spell_check("xx-XX".into(), vec!["a".into()])).unwrap().is_none());
    }
}
