//! Persisted, stable-identity labels. Groups organize tags, never workflow stages.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagBook {
    #[serde(default)]
    pub initialized: bool,
    #[serde(default)]
    next_serial: u64,
    pub groups: Vec<TagGroup>,
    pub tags: Vec<Tag>,
    pub assignments: BTreeMap<String, Vec<String>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagGroup {
    pub id: String,
    pub name: String,
    pub album: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub group: String,
    pub color: u32,
}

impl TagBook {
    pub fn seed_groups(&mut self) {
        self.next_serial = self.next_serial.max(
            self.groups
                .iter()
                .map(|g| g.id.as_str())
                .chain(self.tags.iter().map(|t| t.id.as_str()))
                .filter_map(|id| id.rsplit_once('-')?.1.parse::<u64>().ok())
                .max()
                .unwrap_or(0),
        );
        if self.initialized {
            return;
        }
        self.initialized = true;
        if self.groups.is_empty() {
            self.groups = vec![
                TagGroup {
                    id: "tasks".into(),
                    name: "Task reminders".into(),
                    album: false,
                },
                TagGroup {
                    id: "albums".into(),
                    name: "Albums".into(),
                    album: true,
                },
            ];
        }
    }
    fn allocate_id(&mut self, prefix: &str) -> String {
        // Retain the counter in JSON so deleting a label cannot recycle its identity.
        self.next_serial = self.next_serial.saturating_add(1);
        format!("{prefix}-{}", self.next_serial)
    }
    pub fn validate(&self) -> Result<(), String> {
        let mut group_ids = HashSet::new();
        let mut group_names = HashSet::new();
        for g in &self.groups {
            if g.id.is_empty() || !group_ids.insert(g.id.as_str()) {
                return Err("Duplicate or empty tag group identity".into());
            }
            if g.name.trim().is_empty() || !group_names.insert(g.name.trim().to_lowercase()) {
                return Err("Tag group names must be nonempty and unique".into());
            }
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for t in &self.tags {
            if t.id.is_empty() || !ids.insert(t.id.as_str()) {
                return Err("Duplicate or empty tag identity".into());
            }
            if !group_ids.contains(t.group.as_str()) {
                return Err("A tag points to a missing group".into());
            }
            if t.name.trim().is_empty()
                || !names.insert((t.group.as_str(), t.name.trim().to_lowercase()))
            {
                return Err("Tag names must be nonempty and unique in their group".into());
            }
            if t.color > 0xffffff {
                return Err("Invalid tag color".into());
            }
        }
        for tags in self.assignments.values() {
            let mut seen = HashSet::new();
            for id in tags {
                if !ids.contains(id.as_str()) || !seen.insert(id) {
                    return Err("Invalid or duplicate track tag assignment".into());
                }
            }
        }
        Ok(())
    }
    pub fn create_group(&mut self, name: &str, album: bool) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty()
            || self
                .groups
                .iter()
                .any(|g| g.name.to_lowercase() == name.to_lowercase())
        {
            return Err("Choose a unique group name".into());
        }
        let id = self.allocate_id("group");
        self.groups.push(TagGroup {
            id: id.clone(),
            name: name.into(),
            album,
        });
        Ok(id)
    }
    pub fn update_group(&mut self, id: &str, name: &str, album: bool) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty()
            || self
                .groups
                .iter()
                .any(|g| g.id != id && g.name.to_lowercase() == name.to_lowercase())
        {
            return Err("Choose a unique group name".into());
        }
        let g = self
            .groups
            .iter_mut()
            .find(|g| g.id == id)
            .ok_or("Group no longer exists")?;
        g.name = name.into();
        g.album = album;
        Ok(())
    }
    pub fn remove_group(&mut self, id: &str) {
        let tags: Vec<_> = self
            .tags
            .iter()
            .filter(|t| t.group == id)
            .map(|t| t.id.clone())
            .collect();
        for tag in tags {
            self.remove_tag(&tag);
        }
        self.groups.retain(|g| g.id != id);
    }
    pub fn save_tag(
        &mut self,
        id: Option<&str>,
        name: &str,
        group: &str,
        color: u32,
    ) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() || !self.groups.iter().any(|g| g.id == group) || color > 0xffffff {
            return Err("Enter a name, group, and valid color".into());
        }
        if self.tags.iter().any(|t| {
            Some(t.id.as_str()) != id
                && t.group == group
                && t.name.to_lowercase() == name.to_lowercase()
        }) {
            return Err("This group already has that tag".into());
        }
        if let Some(id) = id {
            let tag = self
                .tags
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or("Tag no longer exists")?;
            tag.name = name.into();
            tag.group = group.into();
            tag.color = color;
            Ok(id.into())
        } else {
            let id = self.allocate_id("tag");
            self.tags.push(Tag {
                id: id.clone(),
                name: name.into(),
                group: group.into(),
                color,
            });
            Ok(id)
        }
    }
    pub fn remove_tag(&mut self, id: &str) {
        self.tags.retain(|t| t.id != id);
        for assigned in self.assignments.values_mut() {
            assigned.retain(|t| t != id);
        }
        self.assignments.retain(|_, tags| !tags.is_empty());
    }
    pub fn toggle(&mut self, track: &str, id: &str) -> Result<(), String> {
        if !self.tags.iter().any(|t| t.id == id) {
            return Err("Tag no longer exists".into());
        }
        let tags = self.assignments.entry(track.into()).or_default();
        if let Some(i) = tags.iter().position(|t| t == id) {
            tags.remove(i);
        } else {
            tags.push(id.into());
        }
        Ok(())
    }
    pub fn for_track(&self, track: &str) -> Vec<&Tag> {
        self.assignments
            .get(track)
            .into_iter()
            .flatten()
            .filter_map(|id| self.tags.iter().find(|t| &t.id == id))
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deleted_identities_and_deleted_default_groups_stay_deleted_after_reload() {
        let mut book = TagBook::default();
        book.seed_groups();
        let old = book.save_tag(None, "Groove", "tasks", 0x81c6ac).unwrap();
        book.remove_tag(&old);
        let mut book: TagBook =
            serde_json::from_str(&serde_json::to_string(&book).unwrap()).unwrap();
        let new = book.save_tag(None, "Extract", "tasks", 0xdfb86d).unwrap();
        assert_ne!(old, new);
        book.remove_group("tasks");
        book.remove_group("albums");
        let mut book: TagBook =
            serde_json::from_str(&serde_json::to_string(&book).unwrap()).unwrap();
        book.seed_groups();
        assert!(book.groups.is_empty());
        assert!(book.tags.is_empty());
    }

    #[test]
    fn stable_tag_edit_and_group_delete_clean_assignments() {
        let mut b = TagBook::default();
        b.seed_groups();
        let id = b.save_tag(None, "Groove", "tasks", 0x81c6ac).unwrap();
        b.toggle("track-1", &id).unwrap();
        b.save_tag(Some(&id), "Arranging", "albums", 0x8ab9df)
            .unwrap();
        assert_eq!(b.for_track("track-1")[0].name, "Arranging");
        let encoded = serde_json::to_vec(&b).unwrap();
        let mut restored: TagBook = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restored, b);
        restored.remove_group("albums");
        assert!(restored.for_track("track-1").is_empty());
        restored.validate().unwrap();
    }
    #[test]
    fn duplicate_names_and_dangling_assignments_are_rejected() {
        let mut b = TagBook::default();
        b.seed_groups();
        b.save_tag(None, "Groove", "tasks", 1).unwrap();
        assert!(b.save_tag(None, " groove ", "tasks", 2).is_err());
        assert!(b.save_tag(None, "", "tasks", 2).is_err());
        assert!(b.create_group("Albums", false).is_err());
        assert!(b.toggle("track", "missing").is_err());
    }
}
