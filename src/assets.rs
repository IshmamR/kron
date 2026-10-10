use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "icons/close.svg" => include_bytes!("../assets/icons/close.svg"),
            "icons/maximize.svg" => include_bytes!("../assets/icons/maximize.svg"),
            "icons/minimize.svg" => include_bytes!("../assets/icons/minimize.svg"),
            "icons/restore.svg" => include_bytes!("../assets/icons/restore.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
