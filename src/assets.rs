use gpui::{AssetSource, SharedString};
use std::borrow::Cow;
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "cadence/volume.svg" => Some(include_bytes!("../assets/ui/volume.svg")),
            "cadence/volume-off.svg" => Some(include_bytes!("../assets/ui/volume-off.svg")),
            "cadence/star.svg" => Some(include_bytes!("../assets/ui/star.svg")),
            "cadence/star-outline.svg" => Some(include_bytes!("../assets/ui/star-outline.svg")),
            "cadence/add-plus.svg" => Some(include_bytes!("../assets/ui/add-plus.svg")),
            "cadence/comments-main.svg" => Some(include_bytes!("../assets/ui/comments-main.svg")),
            "cadence/comments-reference.svg" => {
                Some(include_bytes!("../assets/ui/comments-reference.svg"))
            }
            "cadence/play.svg" => Some(include_bytes!("../assets/ui/play.svg")),
            "cadence/refresh.svg" => Some(include_bytes!("../assets/ui/refresh.svg")),
            "cadence/edit.svg" => Some(include_bytes!("../assets/ui/edit.svg")),
            "cadence/delete.svg" => Some(include_bytes!("../assets/ui/delete.svg")),
            "cadence/add.svg" => Some(include_bytes!("../assets/ui/add.svg")),
            _ => None,
        };
        if let Some(bytes) = bytes {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_component_assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut assets = gpui_component_assets::Assets.list(path)?;
        assets.extend(
            [
                "cadence/volume.svg",
                "cadence/volume-off.svg",
                "cadence/play.svg",
                "cadence/refresh.svg",
                "cadence/edit.svg",
                "cadence/delete.svg",
                "cadence/add.svg",
            ]
            .into_iter()
            .filter(|p| p.starts_with(path))
            .map(Into::into),
        );
        Ok(assets)
    }
}
