use std::borrow::Cow;

use gpui_kit::AssetSource;
use gpui_kit::Result;
use gpui_kit::SharedString;
use gpui_kit::assets::Assets;
use gpui_kit::assets::icon_assets;

/// Asset path of the Dictate mark for `svg().path(..)`.
pub(crate) const DICTATE_MARK: &str = "brand/dictate-mark.svg";

// Lucide icons the app uses beyond the component library's default set.
icon_assets!(
    ExtraIcons,
    [
        ArrowRight, BookA, Clock, House, Puzzle, ScrollText, Settings, Type
    ]
);

/// The application's asset source: the Dictate mark, the extra icons above, then
/// the component library's defaults.
#[derive(Clone, Copy, Debug, Default)]
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == DICTATE_MARK {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../../../packaging/icons/platform/dictate-mark.svg"
            ))));
        }
        match ExtraIcons.load(path)? {
            Some(data) => Ok(Some(data)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = ExtraIcons.list(path)?;
        if DICTATE_MARK.starts_with(path) {
            paths.push(DICTATE_MARK.into());
        }
        paths.extend(Assets.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
