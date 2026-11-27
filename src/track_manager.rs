//! Atomic bulk edits and stable, non-destructive library filtering.
use crate::storage::{self, Library, TrackStage};
use std::collections::BTreeSet;

#[derive(Clone, Default)]
pub struct Filter {
    pub query: String,
    pub tag: Option<String>,
    pub group: Option<String>,
    pub favorites: bool,
    pub stage: Option<TrackStage>,
    pub reverse: bool,
}
#[derive(Clone, Default)]
pub struct BulkEdit {
    pub stage: Option<TrackStage>,
    pub add: BTreeSet<String>,
    pub remove: BTreeSet<String>,
}
impl BulkEdit {
    pub fn is_empty(&self) -> bool {
        self.stage.is_none() && self.add.is_empty() && self.remove.is_empty()
    }
}
pub fn filtered_ids(library: &Library, filter: &Filter) -> Vec<String> {
    let query = filter.query.trim().to_lowercase();
    let mut tracks: Vec<_> = library
        .tracks
        .iter()
        .filter(|t| {
            let tags = library.tags.for_track(&t.id);
            (!filter.favorites || t.favorite)
                && filter.stage.is_none_or(|stage| t.stage == stage)
                && filter
                    .tag
                    .as_ref()
                    .is_none_or(|id| tags.iter().any(|t| &t.id == id))
                && filter
                    .group
                    .as_ref()
                    .is_none_or(|id| tags.iter().any(|t| &t.group == id))
                && (query.is_empty()
                    || t.title.to_lowercase().contains(&query)
                    || t.original_name.to_lowercase().contains(&query)
                    || t.effective_group_key()
                        .is_some_and(|key| key.to_lowercase().contains(&query))
                    || t.next_action.to_lowercase().contains(&query)
                    || tags
                        .iter()
                        .any(|tag| tag.name.to_lowercase().contains(&query)))
        })
        .collect();
    tracks.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    if filter.reverse {
        tracks.reverse();
    }
    tracks.into_iter().map(|t| t.id.clone()).collect()
}
pub fn apply(
    library: &mut Library,
    ids: &BTreeSet<String>,
    edit: &BulkEdit,
) -> Result<bool, String> {
    if ids.is_empty() {
        return Err("Select tracks first".into());
    }
    if ids
        .iter()
        .any(|id| !library.tracks.iter().any(|t| &t.id == id))
    {
        return Err("A selected track no longer exists".into());
    }
    if edit
        .add
        .iter()
        .chain(&edit.remove)
        .any(|id| !library.tags.tags.iter().any(|t| &t.id == id))
    {
        return Err("A selected tag no longer exists".into());
    }
    if !edit.add.is_disjoint(&edit.remove) {
        return Err("A tag cannot be added and removed together".into());
    }
    let mut staged = library.clone();
    let mut changed = false;
    for id in ids {
        if let Some(stage) = edit.stage {
            changed |= storage::set_track_stage(&mut staged, id, stage)?;
        }
        let assigned = staged.tags.assignments.entry(id.clone()).or_default();
        let old = assigned.clone();
        assigned.retain(|tag| !edit.remove.contains(tag));
        for tag in &edit.add {
            if !assigned.contains(tag) {
                assigned.push(tag.clone());
            }
        }
        changed |= *assigned != old;
    }
    staged.tags.assignments.retain(|_, tags| !tags.is_empty());
    storage::normalize_planner_order(&mut staged);
    storage::validate_library_identity(&staged)?;
    if changed {
        *library = staged;
    }
    Ok(changed)
}
pub fn remove(library: &mut Library, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        return Err("Select tracks first".into());
    }
    let mut staged = library.clone();
    for id in ids {
        let selected = staged.selected_track_id.as_ref() == Some(id);
        let (index, _) = storage::remove_track(&mut staged, id)?;
        staged.tags.assignments.remove(id);
        if selected {
            staged.selected_track_id = storage::selection_after_removal(&staged, index);
        }
    }
    storage::normalize_planner_order(&mut staged);
    storage::validate_library_identity(&staged)?;
    *library = staged;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn library() -> Library {
        let mut library: Library = serde_json::from_str(r#"{"tracks":[{"id":"one","title":"Zulu","original_name":"one.wav","path":"/one.wav","size":1,"favorite":true,"stage":"sound-design","notes":[]},{"id":"two","title":"Alpha","original_name":"two.wav","path":"/two.wav","size":1,"favorite":false,"stage":"production","notes":[]}],"selected_track_id":"one"}"#).unwrap();
        library.tags.seed_groups();
        library
    }
    #[test]
    fn bulk_changes_are_atomic_idempotent_and_persisted() {
        let mut library = library();
        let tag = library
            .tags
            .save_tag(None, "Night shift", "albums", 0xad9abb)
            .unwrap();
        let ids = BTreeSet::from(["one".into(), "two".into()]);
        let mut edit = BulkEdit {
            stage: Some(TrackStage::Mixdown),
            add: BTreeSet::from([tag.clone()]),
            ..Default::default()
        };
        assert!(apply(&mut library, &ids, &edit).unwrap());
        assert!(!apply(&mut library, &ids, &edit).unwrap());
        let encoded = serde_json::to_string(&library).unwrap();
        edit.add.insert("missing".into());
        assert!(apply(&mut library, &ids, &edit).is_err());
        assert_eq!(encoded, serde_json::to_string(&library).unwrap());
        let mut library: Library = serde_json::from_str(&encoded).unwrap();
        assert!(
            library
                .tracks
                .iter()
                .all(|t| t.stage == TrackStage::Mixdown)
        );
        assert_eq!(library.tags.for_track("two")[0].id, tag);
        assert!(
            apply(
                &mut library,
                &ids,
                &BulkEdit {
                    remove: BTreeSet::from([tag]),
                    ..Default::default()
                }
            )
            .unwrap()
        );
        assert!(library.tags.assignments.is_empty());
        assert!(
            apply(
                &mut library,
                &BTreeSet::from(["missing".into()]),
                &BulkEdit::default()
            )
            .is_err()
        );
    }
    #[test]
    fn bulk_removal_is_atomic_and_cleans_selection_and_tags() {
        let mut library = library();
        let tag = library.tags.save_tag(None, "Groove", "tasks", 1).unwrap();
        library.tags.toggle("one", &tag).unwrap();
        let before = serde_json::to_string(&library).unwrap();
        assert!(remove(&mut library, &["one".into(), "missing".into()]).is_err());
        assert_eq!(before, serde_json::to_string(&library).unwrap());
        remove(&mut library, &["one".into()]).unwrap();
        assert_eq!(library.selected_track_id.as_deref(), Some("two"));
        assert!(!library.tags.assignments.contains_key("one"));
        remove(&mut library, &["two".into()]).unwrap();
        assert!(library.selected_track_id.is_none());
        assert!(library.tracks.is_empty());
    }
    #[test]
    fn filters_combine_without_changing_library_order() {
        let mut library = library();
        let tag = library
            .tags
            .save_tag(None, "Night shift", "albums", 1)
            .unwrap();
        library.tags.toggle("one", &tag).unwrap();
        assert_eq!(
            filtered_ids(&library, &Filter::default()),
            vec!["two", "one"]
        );
        let filter = Filter {
            query: "night".into(),
            group: Some("albums".into()),
            favorites: true,
            stage: Some(TrackStage::Backlog),
            ..Default::default()
        };
        assert_eq!(filtered_ids(&library, &filter), vec!["one"]);
        assert_eq!(library.tracks.first().unwrap().id, "one");
    }
}
