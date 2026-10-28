use windows::Win32::Globalization::{
    GetUserDefaultLocaleName, GetUserPreferredUILanguages, MUI_LANGUAGE_NAME,
};
use windows::core::PWSTR;

pub fn get_current_locale_identifier() -> String {
    let mut buffer = [0_u16; 85];
    let size = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    if size > 1 {
        String::from_utf16_lossy(&buffer[..size as usize - 1])
    } else {
        String::new()
    }
}

pub fn get_preferred_languages() -> Vec<hypr_language::Language> {
    let mut count = 0;
    let mut size = 0;
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut size) }
        .is_err()
    {
        return get_current_locale_identifier()
            .parse()
            .ok()
            .into_iter()
            .collect();
    }
    let mut buffer = vec![0_u16; size as usize];
    if unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut size,
        )
    }
    .is_err()
    {
        return get_current_locale_identifier()
            .parse()
            .ok()
            .into_iter()
            .collect();
    }
    buffer
        .split(|c| *c == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| String::from_utf16_lossy(part).parse().ok())
        .collect()
}
