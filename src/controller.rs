//! GUI-independent application state and serialized background I/O.
use crate::{
    audio,
    source::SourceProvenance,
    storage::{self, Library, Note, TrackStage},
    transport::AudioTransport,
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Main,
    Reference,
}
impl Source {
    pub fn index(self) -> usize {
        if self == Self::Main { 0 } else { 1 }
    }
}
#[derive(Clone, Debug)]
pub enum ImportKind {
    Main,
    AddRevision(String),
    Relink(String, Option<String>),
    #[allow(dead_code)]
    Replace(String),
    Reference(String),
    Catalog,
    ReplaceReference(PathBuf),
}
enum Work {
    Load,
    Recover,
    Barrier(Arc<AtomicBool>),
    Save(u64, Library),
    Import(Library, Vec<PathBuf>, ImportKind),
    Decode(u64, Source, PathBuf, SourceProvenance),
}
enum Event {
    Loaded(Result<Library, String>),
    Saved(u64, Result<storage::PersistenceOutcome, String>),
    Imported(Result<(Library, Option<String>), String>),
    Decoded(u64, Source, Result<audio::DecodedAudioFile, String>),
}

const MAX_RAM_ANALYSES: usize = 6;

#[derive(Default)]
struct AnalysisCache {
    entries: VecDeque<audio::DecodedAudioFile>,
}

impl AnalysisCache {
    fn get(
        &mut self,
        path: &std::path::Path,
        expected_proof: Option<&crate::source::AudioSourceProof>,
        should_cancel: &impl Fn() -> bool,
    ) -> Result<Option<audio::DecodedAudioFile>, String> {
        if should_cancel() {
            return Err(String::from("cancelled"));
        }
        let Some(index) = self.entries.iter().position(|entry| entry.path() == path) else {
            return Ok(None);
        };
        let entry = self.entries.remove(index).expect("entry index was found");
        let proof_matches = expected_proof.is_none_or(|proof| entry.source_proof() == proof);
        if !proof_matches || entry.validate_source().is_err() {
            return Ok(None);
        }
        if should_cancel() {
            self.entries.push_front(entry);
            return Err(String::from("cancelled"));
        }
        let cached = entry.clone();
        self.entries.push_front(entry);
        Ok(Some(cached))
    }

    fn insert(&mut self, decoded: audio::DecodedAudioFile) {
        self.entries.retain(|entry| {
            entry.path() != decoded.path() || entry.source_proof() != decoded.source_proof()
        });
        self.entries.push_front(decoded);
        self.entries.truncate(MAX_RAM_ANALYSES);
    }

    fn decode(
        &mut self,
        path: &std::path::Path,
        expected: Option<&SourceProvenance>,
        should_cancel: impl Fn() -> bool,
    ) -> Result<audio::DecodedAudioFile, String> {
        let expected_proof = expected.and_then(SourceProvenance::verified_proof);
        if let Some(decoded) = self.get(path, expected_proof, &should_cancel)? {
            return Ok(decoded);
        }

        if let Some(proof) = expected_proof {
            let cache_path = storage::waveform_cache_path(path, proof);
            if let Some(decoded) =
                audio::load_decoded_audio_cache(path, &cache_path, proof, &should_cancel)?
            {
                self.insert(decoded.clone());
                return Ok(decoded);
            }
        }

        let decoded = audio::decode_audio_file_with_cancellation(path, &should_cancel)?;
        if let Some(proof) = expected_proof
            && proof != decoded.source_proof()
        {
            return Err("Audio source changed. Replace or re-import the file before reviewing its comments.".into());
        }
        if should_cancel() {
            return Err(String::from("cancelled"));
        }
        let cache_path = storage::waveform_cache_path(path, decoded.source_proof());
        let _ = audio::write_waveform_cache_if_unchanged_with_cancellation(
            path,
            &cache_path,
            &decoded.source_ticket(),
            decoded.waveform(),
            &should_cancel,
        );
        if should_cancel() {
            return Err(String::from("cancelled"));
        }
        self.insert(decoded.clone());
        Ok(decoded)
    }
}

pub struct Controller {
    pub library: Library,
    pub ready: bool,
    pub importing: bool,
    pub error: Option<String>,
    pub warning: Option<String>,
    pub generation: u64,
    pub decoded: [Option<audio::DecodedAudioFile>; 2],
    pub analyzing: [bool; 2],
    pub missing_source: [bool; 2],
    pub transports: [AudioTransport; 2],
    pub source: Source,
    pub position: u64,
    pub playing: bool,
    pub volume: f32,
    pub muted: bool,
    pub matching: bool,
    pub loops: [Option<(f32, f32)>; 2],
    pending_tokens: [u64; 2],
    live_states: [crate::transport::LiveFrameState; 2],
    pub revision: u64,
    pub saved_revision: u64,
    jobs: Sender<Work>,
    events: Receiver<Event>,
    epoch: Arc<AtomicU64>,
}
impl Controller {
    pub fn new() -> Self {
        let (jobs, incoming) = mpsc::channel();
        let (outgoing, events) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = epoch.clone();
        std::thread::Builder::new()
            .name("cadence-library-worker".into())
            .spawn(move || {
                let mut analysis_cache = AnalysisCache::default();
                while let Ok(work) = incoming.recv() {
                    let event = match work {
                        Work::Barrier(done) => {
                            done.store(true, Ordering::Release);
                            continue;
                        }
                        Work::Load => Event::Loaded(storage::load_library()),
                        Work::Recover => Event::Loaded(
                            storage::preserve_unreadable_library_and_start_fresh()
                                .map(|_| Library::default()),
                        ),
                        Work::Save(rev, library) => {
                            Event::Saved(rev, storage::persist_library(&library))
                        }
                        Work::Import(mut library, paths, kind) => {
                            let mut errors = Vec::new();
                            let mut warnings = Vec::new();
                            let total_paths = paths.len();
                            for path in paths {
                                let filename = path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("audio file")
                                    .to_string();
                                let decoded = analysis_cache.decode(&path, None, || false);
                                match decoded {
                                    Ok(decoded) => match &kind {
                                        ImportKind::Main => {
                                            match storage::import_bounce_into_library(
                                                library.clone(),
                                                decoded,
                                            ) {
                                                Ok(saved) => {
                                                    library = saved.value.0;
                                                    match saved.value.1 {
                                                        storage::BounceImportOutcome::CollisionFlagged {
                                                            build,
                                                            ..
                                                        } => warnings.push(format!(
                                                            "Collision on b{build}: {filename} differs from the existing build and was flagged without overwriting."
                                                        )),
                                                        storage::BounceImportOutcome::DuplicateIgnored {
                                                            ..
                                                        } if total_paths == 1 => warnings.push(
                                                            format!("{filename} is already imported; duplicate drop ignored."),
                                                        ),
                                                        _ => {}
                                                    }
                                                    if let Some(w) =
                                                        saved.outcome.durability_warning()
                                                    {
                                                        warnings.push(w);
                                                    }
                                                }
                                                Err(e) => errors.push(e),
                                            }
                                        }
                                        ImportKind::AddRevision(id) => {
                                            match storage::add_revision_to_track(
                                                library.clone(),
                                                id,
                                                decoded,
                                            ) {
                                                Ok(saved) => {
                                                    library = saved.value.0;
                                                    if let storage::BounceImportOutcome::CollisionFlagged {
                                                        build,
                                                        ..
                                                    } = saved.value.1
                                                    {
                                                        warnings.push(format!(
                                                            "Collision on b{build}: {filename} differs from the existing build and was flagged without overwriting."
                                                        ));
                                                    }
                                                    if let Some(w) =
                                                        saved.outcome.durability_warning()
                                                    {
                                                        warnings.push(w);
                                                    }
                                                }
                                                Err(e) => errors.push(e),
                                            }
                                        }
                                        ImportKind::Relink(id, rev_id) => {
                                            match storage::relink_track_revision(
                                                library.clone(),
                                                id,
                                                rev_id.as_deref(),
                                                decoded,
                                            ) {
                                                Ok(saved) => {
                                                    library = saved.value;
                                                    if let Some(w) =
                                                        saved.outcome.durability_warning()
                                                    {
                                                        warnings.push(w);
                                                    }
                                                }
                                                Err(e) => errors.push(e),
                                            }
                                        }
                                        ImportKind::Replace(id) => {
                                            match storage::replace_track(
                                                library.clone(),
                                                id,
                                                decoded,
                                            ) {
                                                Ok(saved) => {
                                                    library = saved.value;
                                                    if let Some(w) =
                                                        saved.outcome.durability_warning()
                                                    {
                                                        warnings.push(w);
                                                    }
                                                }
                                                Err(e) => errors.push(e),
                                            }
                                        }
                                        ImportKind::Reference(id) => {
                                            match storage::set_reference_track(
                                                library.clone(),
                                                id,
                                                decoded,
                                            ) {
                                                Ok(saved) => {
                                                    library = saved.value;
                                                    if let Some(w) =
                                                        saved.outcome.durability_warning()
                                                    {
                                                        warnings.push(w);
                                                    }
                                                }
                                                Err(e) => errors.push(e),
                                            }
                                        }
                                        ImportKind::Catalog => {
                                            match storage::add_reference_track(
                                                library.clone(),
                                                decoded,
                                            ) {
                                                Ok(saved) => {
                                                    library = saved.value;
                                                    if let Some(w) =
                                                        saved.outcome.durability_warning()
                                                    {
                                                        warnings.push(w);
                                                    }
                                                }
                                                Err(e) => errors.push(e),
                                            }
                                        }
                                        ImportKind::ReplaceReference(path) => {
                                            let proof = library
                                                .reference_tracks
                                                .iter()
                                                .find(|t| &t.path == path)
                                                .and_then(|t| t.source_proof.verified_proof())
                                                .cloned();
                                            match storage::replace_reference_track(
                                                library.clone(),
                                                path,
                                                proof.as_ref(),
                                                decoded,
                                            ) {
                                                Ok(saved) => {
                                                    library = saved.value;
                                                    if let Some(w) =
                                                        saved.outcome.durability_warning()
                                                    {
                                                        warnings.push(w);
                                                    }
                                                }
                                                Err(e) => errors.push(e),
                                            }
                                        }
                                    },
                                    Err(e) => errors.push(e),
                                }
                            }
                            // Partial batch success is never discarded. Report errors separately.
                            if !errors.is_empty() {
                                let _ = outgoing.send(Event::Saved(0, Err(errors.join("\n"))));
                            }
                            let warning = if warnings.is_empty() {
                                None
                            } else {
                                Some(warnings.join("\n"))
                            };
                            Event::Imported(Ok((library, warning)))
                        }
                        Work::Decode(generation, src, path, expected) => {
                            let result = analysis_cache.decode(&path, Some(&expected), || {
                                worker_epoch.load(Ordering::Acquire) != generation
                            });
                            Event::Decoded(generation, src, result)
                        }
                    };
                    if outgoing.send(event).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn library worker");
        let mut value = Self {
            library: Library::default(),
            ready: false,
            importing: false,
            error: None,
            warning: None,
            generation: 0,
            decoded: [None, None],
            analyzing: [false, false],
            missing_source: [false, false],
            transports: [
                AudioTransport::try_spawn()
                    .unwrap_or_else(|e| AudioTransport::unavailable(Some(e))),
                AudioTransport::try_spawn()
                    .unwrap_or_else(|e| AudioTransport::unavailable(Some(e))),
            ],
            source: Source::Main,
            position: 0,
            playing: false,
            volume: 0.8,
            muted: false,
            matching: false,
            loops: [None, None],
            pending_tokens: [0, 0],
            live_states: [Default::default(); 2],
            revision: 0,
            saved_revision: 0,
            jobs,
            events,
            epoch,
        };
        value.send(Work::Load);
        value
    }
    fn send(&mut self, work: Work) {
        if self.jobs.send(work).is_err() {
            self.error = Some("Library worker is unavailable".into());
        }
    }
    pub fn shutdown_barrier(&mut self) -> Arc<AtomicBool> {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        let done = Arc::new(AtomicBool::new(false));
        if self.jobs.send(Work::Barrier(done.clone())).is_err() {
            self.error = Some("Library worker is unavailable".into());
            done.store(true, Ordering::Release);
        }
        done
    }
    pub fn dirty(&self) -> bool {
        self.revision != self.saved_revision
    }
    pub fn selected(&self) -> Option<&storage::Track> {
        let id = self.library.selected_track_id.as_ref()?;
        self.library.tracks.iter().find(|t| &t.id == id)
    }
    pub fn can_edit(&mut self) -> bool {
        if !self.ready || self.importing {
            self.error = Some("Wait for the library operation to finish.".into());
            false
        } else {
            true
        }
    }
    pub fn persist(&mut self) {
        self.revision += 1;
        self.send(Work::Save(self.revision, self.library.clone()));
    }
    pub fn retry_save(&mut self) {
        self.error = None;
        self.send(Work::Save(self.revision, self.library.clone()));
    }
    pub fn retry_load(&mut self) {
        self.error = None;
        self.send(Work::Load);
    }
    pub fn recover(&mut self) {
        self.error = None;
        self.send(Work::Recover);
    }
    pub fn import(&mut self, paths: Vec<PathBuf>, kind: ImportKind) {
        if paths.is_empty() || !self.can_edit() {
            return;
        }
        if self.dirty() {
            self.error = Some(
                "Wait for the library to save before importing; retry a failed save first.".into(),
            );
            return;
        }
        self.importing = true;
        self.error = None;
        self.send(Work::Import(self.library.clone(), paths, kind));
    }
    pub fn select(&mut self, id: String) {
        if !self.can_edit() || self.library.selected_track_id.as_ref() == Some(&id) {
            return;
        }
        if !self.library.tracks.iter().any(|t| t.id == id) {
            return;
        }
        self.library.selected_track_id = Some(id);
        self.load_selected();
        self.persist();
    }
    pub fn load_selected(&mut self) {
        self.generation += 1;
        self.epoch.store(self.generation, Ordering::Release);
        self.decoded = [None, None];
        self.analyzing = [false, false];
        self.missing_source = [false, false];
        self.loops = [None, None];
        self.position = 0;
        self.playing = false;
        self.source = Source::Main;
        for transport in &self.transports {
            let _ = transport.unload(self.generation);
        }
        if let Some(track) = self.selected().cloned() {
            self.missing_source[0] = track.is_source_missing();
            self.analyzing[0] = true;
            self.send(Work::Decode(
                self.generation,
                Source::Main,
                track.path,
                track.source_proof,
            ));
            if let Some(path) = track.reference_path {
                self.missing_source[1] = !path.is_file();
                let proof = self
                    .library
                    .reference_tracks
                    .iter()
                    .find(|r| r.path == path)
                    .map(|r| r.source_proof.clone())
                    .unwrap_or_default();
                self.analyzing[1] = true;
                self.send(Work::Decode(
                    self.generation,
                    Source::Reference,
                    path,
                    proof,
                ));
            }
        }
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.events.try_recv() {
            changed = true;
            match event {
                Event::Loaded(result) => match result {
                    Ok(mut lib) => {
                        lib.tags.seed_groups();
                        self.library = lib;
                        self.ready = true;
                        self.error = None;
                        if self.selected().is_none() {
                            self.library.selected_track_id =
                                self.library.tracks.first().map(|t| t.id.clone());
                        }
                        self.load_selected();
                    }
                    Err(e) => {
                        self.ready = false;
                        self.error = Some(e);
                    }
                },
                Event::Saved(rev, result) => match result {
                    Ok(outcome) => {
                        self.saved_revision = self.saved_revision.max(rev);
                        self.warning = outcome.durability_warning();
                    }
                    Err(e) => self.error = Some(e),
                },
                Event::Imported(result) => {
                    self.importing = false;
                    match result {
                        Ok((lib, warning)) => {
                            self.library = lib;
                            self.saved_revision = self.revision;
                            self.warning = warning;
                            self.load_selected();
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Event::Decoded(generation, src, result) => {
                    if generation != self.generation {
                        continue;
                    }
                    self.analyzing[src.index()] = false;
                    match result {
                        Ok(decoded) => {
                            self.missing_source[src.index()] = false;
                            let duration = decoded.waveform().duration_millis;
                            if let Err(e) = self.transports[src.index()].load(
                                generation,
                                decoded.source_ticket(),
                                duration,
                            ) {
                                self.error = Some(e);
                            }
                            self.decoded[src.index()] = Some(decoded);
                            self.apply_gain();
                        }
                        Err(e) => {
                            let is_missing = match src {
                                Source::Main => {
                                    self.selected().is_some_and(storage::Track::is_source_missing)
                                }
                                Source::Reference => self
                                    .selected()
                                    .and_then(|t| t.reference_path.as_ref())
                                    .is_some_and(|p| !p.is_file()),
                            };
                            self.missing_source[src.index()] = is_missing;
                            if !is_missing {
                                self.error = Some(e);
                            }
                        }
                    }
                }
            }
        }
        let snapshot = self.transports[self.source.index()].snapshot();
        if snapshot.generation == self.generation
            && snapshot.acknowledged_token >= self.pending_tokens[self.source.index()]
        {
            if self.position != snapshot.position_millis || self.playing != snapshot.playing {
                changed = true;
            }
            self.position = snapshot.position_millis;
            self.playing = snapshot.playing;
            if self.playing
                && let Some((a, b)) = self.loops[self.source.index()]
                && self.position >= self.duration(self.source).mul_f32(b)
            {
                self.seek(self.duration(self.source).mul_f32(a), true);
            }
        }
        for (i, t) in self.transports.iter().enumerate() {
            let state = t.live_frame_state();
            if state != self.live_states[i] {
                self.live_states[i] = state;
                changed = true;
            }
            if let Some(warning) = t.take_analysis_warning(self.generation) {
                self.warning = Some(warning);
                changed = true;
            }
            if let Some(e) = t.take_error(self.generation) {
                self.error = Some(e);
                changed = true;
            }
        }
        changed
    }
    pub fn duration(&self, src: Source) -> u64 {
        self.decoded[src.index()]
            .as_ref()
            .map(|d| d.waveform().duration_millis)
            .unwrap_or(0)
    }
    pub fn waveform(&self, src: Source) -> Option<&audio::WaveformData> {
        self.decoded[src.index()].as_ref().map(|d| d.waveform())
    }
    pub fn toggle_play(&mut self, source: Source) {
        if self.decoded[source.index()].is_none() {
            return;
        }
        if self.source != source {
            self.switch_source(source);
            self.seek(self.position, true);
            return;
        }
        let result = if self.playing {
            self.transports[source.index()].pause(self.generation)
        } else {
            self.transports[source.index()].play(self.generation)
        };
        if let Err(e) = result {
            self.error = Some(e);
        }
    }
    pub fn switch_source(&mut self, source: Source) {
        let position = self.position;
        let resume = self.playing;
        let _ = self.transports[self.source.index()].pause(self.generation);
        self.source = source;
        self.apply_gain();
        self.seek(position, resume);
    }
    pub fn seek(&mut self, position: u64, resume: bool) {
        for src in [Source::Main, Source::Reference] {
            if self.decoded[src.index()].is_some() {
                match self.transports[src.index()].seek(
                    self.generation,
                    position,
                    self.duration(src),
                    resume && src == self.source,
                ) {
                    Ok(token) => self.pending_tokens[src.index()] = token,
                    Err(e) => self.error = Some(e),
                }
            }
        }
        self.position = position.min(self.duration(self.source));
    }
    pub fn stop(&mut self) {
        self.seek(0, false);
    }
    pub fn apply_gain(&self) {
        let main = if self.muted { 0.0 } else { self.volume };
        let db = if self.matching {
            audio::loudness_match_gain_db(
                self.waveform(Source::Main).and_then(|w| w.integrated_lufs),
                self.waveform(Source::Reference)
                    .and_then(|w| w.integrated_lufs),
            )
            .unwrap_or(0.0)
        } else {
            0.0
        };
        self.transports[0].set_output_gain(main);
        self.transports[1].set_output_gain(main * audio::linear_gain_for_db(db));
    }
    pub fn set_loop(&mut self, src: Source, a: f32, b: f32) {
        let (a, b) = (a.min(b).clamp(0.0, 1.0), a.max(b).clamp(0.0, 1.0));
        self.loops[src.index()] = if b - a > 0.001 { Some((a, b)) } else { None };
    }
    pub fn favorite(&mut self, id: &str) {
        if !self.can_edit() {
            return;
        }
        if let Some(t) = self.library.tracks.find_mut(|t| t.id == id) {
            t.favorite = !t.favorite;
            self.persist();
        }
    }
    pub fn stage(&mut self, id: &str, stage: TrackStage, slot: usize) {
        if !self.can_edit() {
            return;
        }
        match storage::move_track_to_planner_slot(&mut self.library, id, stage, slot) {
            Ok(_) => self.persist(),
            Err(e) => self.error = Some(e),
        }
    }
    pub fn remove(&mut self, id: &str) {
        if !self.can_edit() {
            return;
        }
        let selected = self.library.selected_track_id.as_deref() == Some(id);
        match storage::remove_track(&mut self.library, id) {
            Ok((index, _)) => {
                self.library.tags.assignments.remove(id);
                if selected {
                    self.library.selected_track_id =
                        storage::selection_after_removal(&self.library, index);
                    self.load_selected();
                }
                self.persist();
            }
            Err(e) => self.error = Some(e),
        }
    }
    pub fn remove_bulk(&mut self, ids: &[String]) {
        if !self.can_edit() {
            return;
        }
        let selected = self.library.selected_track_id.clone();
        match crate::track_manager::remove(&mut self.library, ids) {
            Ok(()) => {
                if selected != self.library.selected_track_id {
                    self.load_selected();
                }
                self.persist();
            }
            Err(e) => self.error = Some(e),
        }
    }
    pub fn assign_reference(&mut self, path: PathBuf) {
        if !self.can_edit() {
            return;
        }
        if let Some(id) = self.library.selected_track_id.clone() {
            match storage::set_reference_track_selection(&mut self.library, &id, path) {
                Ok(_) => {
                    self.load_selected();
                    self.persist();
                }
                Err(e) => self.error = Some(e),
            }
        }
    }
    pub fn provenance_known(&self, src: Source) -> bool {
        if self.decoded[src.index()].is_none() {
            return false;
        }
        match src {
            Source::Main => self
                .selected()
                .is_some_and(|t| t.source_proof.verified_proof().is_some()),
            Source::Reference => self
                .selected()
                .and_then(|t| t.reference_path.as_ref())
                .and_then(|p| self.library.reference_tracks.iter().find(|r| &r.path == p))
                .is_some_and(|r| r.source_proof.verified_proof().is_some()),
        }
    }
    pub fn bind_historical(&mut self, src: Source) {
        if !self.can_edit() {
            return;
        }
        let Some(decoded) = self.decoded[src.index()].as_ref() else {
            return;
        };
        if let Err(error) = decoded.validate_source() {
            self.error = Some(error);
            return;
        }
        let proof = decoded.source_proof().clone();
        let path = decoded.path().to_path_buf();
        let result = match src {
            Source::Main => self
                .library
                .selected_track_id
                .clone()
                .ok_or_else(|| "No track selected".into())
                .and_then(|id| {
                    storage::bind_main_source_proof(&mut self.library, &id, &path, proof)
                }),
            Source::Reference => {
                storage::bind_reference_source_proof(&mut self.library, &path, proof)
            }
        };
        match result {
            Ok(_) => self.persist(),
            Err(e) => self.error = Some(e),
        }
    }
    pub fn notes(&self, src: Source) -> Vec<Note> {
        if !self.provenance_known(src) {
            return Vec::new();
        }
        match src {
            Source::Main => self
                .selected()
                .map(|t| t.notes.to_vec())
                .unwrap_or_default(),
            Source::Reference => self
                .selected()
                .and_then(|t| t.reference_path.as_ref())
                .and_then(|p| self.library.reference_tracks.iter().find(|r| &r.path == p))
                .map(|r| r.notes.to_vec())
                .unwrap_or_default(),
        }
    }
    fn notes_mut(&mut self, src: Source) -> Option<&mut storage::SharedVec<Note>> {
        let track = self.selected()?.clone();
        match src {
            Source::Main => self
                .library
                .tracks
                .find_mut(|t| t.id == track.id)
                .map(|t| &mut t.notes),
            Source::Reference => {
                let path = track.reference_path?;
                self.library
                    .reference_tracks
                    .find_mut(|r| r.path == path)
                    .map(|r| &mut r.notes)
            }
        }
    }
    pub fn save_note(&mut self, src: Source, id: Option<&str>, time: u64, body: &str) -> bool {
        if !self.can_edit() || !self.provenance_known(src) {
            self.error = Some("Verify the audio source before editing its comments.".into());
            return false;
        }
        if let Some(decoded) = &self.decoded[src.index()]
            && let Err(error) = decoded.validate_source()
        {
            self.error = Some(error);
            return false;
        }
        if body.trim().is_empty() {
            self.error = Some("Write a comment first.".into());
            return false;
        }
        let time = time.min(self.duration(src));
        let Some(notes) = self.notes_mut(src) else {
            return false;
        };
        if let Some(id) = id {
            let Some(note) = notes.iter_mut().find(|n| n.id == id) else {
                self.error = Some("Comment no longer exists.".into());
                return false;
            };
            note.body = body.trim().into();
            note.time_millis = time;
        } else {
            let id = storage::allocate_note_id(notes);
            notes.push(Note {
                id,
                time_millis: time,
                body: body.trim().into(),
                done: false,
            });
        }
        notes.as_mut_vec().sort_by_key(|n| n.time_millis);
        if src == Source::Main
            && let Some(track_id) = self.library.selected_track_id.clone()
        {
            storage::sync_active_revision_notes(&mut self.library, &track_id);
        }
        self.persist();
        true
    }
    pub fn delete_note(&mut self, src: Source, id: &str) {
        if !self.can_edit() {
            return;
        }
        if let Some(notes) = self.notes_mut(src) {
            notes.retain(|n| n.id != id);
            if src == Source::Main
                && let Some(track_id) = self.library.selected_track_id.clone()
            {
                storage::sync_active_revision_notes(&mut self.library, &track_id);
            }
            self.persist();
        }
    }
    pub fn resolve_note(&mut self, src: Source, id: &str) {
        if !self.can_edit() {
            return;
        }
        if let Some(notes) = self.notes_mut(src)
            && let Some(n) = notes.iter_mut().find(|n| n.id == id)
        {
            n.done = !n.done;
            if src == Source::Main
                && let Some(track_id) = self.library.selected_track_id.clone()
            {
                storage::sync_active_revision_notes(&mut self.library, &track_id);
            }
            self.persist();
        }
    }
    pub fn select_revision(&mut self, track_id: &str, revision_id: &str) {
        if !self.can_edit() {
            return;
        }
        match storage::select_track_revision(&mut self.library, track_id, revision_id) {
            Ok(changed) => {
                if changed {
                    self.error = None;
                    self.load_selected();
                    self.persist();
                }
            }
            Err(e) => self.error = Some(e),
        }
    }
    pub fn set_title(&mut self, track_id: &str, title: &str) -> bool {
        if !self.can_edit() {
            return false;
        }
        match storage::set_track_title(&mut self.library, track_id, title) {
            Ok(changed) => {
                if changed {
                    self.persist();
                }
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }
    pub fn set_next_action(&mut self, track_id: &str, next_action: &str) -> bool {
        if !self.can_edit() {
            return false;
        }
        match storage::set_track_next_action(&mut self.library, track_id, next_action) {
            Ok(changed) => {
                if changed {
                    self.persist();
                }
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }
    pub fn set_revision_feedback(
        &mut self,
        track_id: &str,
        revision_id: Option<&str>,
        note: &str,
    ) -> bool {
        if !self.can_edit() {
            return false;
        }
        match storage::set_revision_feedback_note(&mut self.library, track_id, revision_id, note) {
            Ok(changed) => {
                if changed {
                    self.persist();
                }
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }
    pub fn assign_track_as_revision(&mut self, source_track_id: &str, target_track_id: &str) -> bool {
        if !self.can_edit() {
            return false;
        }
        match storage::assign_standalone_track_as_revision(
            &mut self.library,
            source_track_id,
            target_track_id,
        ) {
            Ok(outcome) => {
                if let storage::BounceImportOutcome::CollisionFlagged { build, .. } = outcome {
                    self.warning = Some(format!(
                        "Collision on b{build}: assigned audio differs from the existing build and was flagged without overwriting."
                    ));
                }
                self.load_selected();
                self.persist();
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }
    pub fn resolve_collision_keep(&mut self, track_id: &str, collision_id: &str) {
        if !self.can_edit() {
            return;
        }
        match storage::resolve_collision_keep_existing(&mut self.library, track_id, collision_id) {
            Ok(()) => {
                self.warning = None;
                self.persist();
            }
            Err(e) => self.error = Some(e),
        }
    }
    pub fn resolve_collision_replace(&mut self, track_id: &str, collision_id: &str) {
        if !self.can_edit() {
            return;
        }
        match storage::resolve_collision_replace_build(&mut self.library, track_id, collision_id) {
            Ok(()) => {
                self.warning = None;
                self.load_selected();
                self.persist();
            }
            Err(e) => self.error = Some(e),
        }
    }
    pub fn resolve_collision_split(&mut self, track_id: &str, collision_id: &str) {
        if !self.can_edit() {
            return;
        }
        match storage::resolve_collision_import_as_new_track(
            &mut self.library,
            track_id,
            collision_id,
        ) {
            Ok(_) => {
                self.warning = None;
                self.load_selected();
                self.persist();
            }
            Err(e) => self.error = Some(e),
        }
    }
}
trait MulRatio {
    fn mul_f32(self, r: f32) -> u64;
}
impl MulRatio for u64 {
    fn mul_f32(self, r: f32) -> u64 {
        (self as f64 * r as f64) as u64
    }
}

pub const TICK: Duration = Duration::from_millis(16);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Snapshot;

    fn isolated() -> (Controller, Receiver<Work>, Sender<Event>) {
        let (jobs, incoming) = mpsc::channel();
        let (outgoing, events) = mpsc::channel();
        let library = serde_json::from_str(r#"{"tracks":[{"id":"one","title":"One","original_name":"one.wav","path":"/one.wav","size":1,"favorite":false,"stage":"sound-design","notes":[]},{"id":"two","title":"Two","original_name":"two.wav","path":"/two.wav","size":1,"favorite":false,"stage":"production","notes":[]}],"selected_track_id":"one"}"#).unwrap();
        (
            Controller {
                library,
                ready: true,
                importing: false,
                error: None,
                warning: None,
                generation: 4,
                decoded: [None, None],
                analyzing: [false, false],
                missing_source: [false, false],
                transports: [
                    AudioTransport::unavailable(None),
                    AudioTransport::unavailable(None),
                ],
                source: Source::Main,
                position: 0,
                playing: false,
                volume: 0.8,
                muted: false,
                matching: false,
                loops: [None, None],
                pending_tokens: [0, 0],
                live_states: [Default::default(); 2],
                revision: 0,
                saved_revision: 0,
                jobs,
                events,
                epoch: Arc::new(AtomicU64::new(4)),
            },
            incoming,
            outgoing,
        )
    }

    #[test]
    fn disconnected_worker_does_not_block_application_shutdown() {
        let (mut core, jobs, _) = isolated();
        drop(jobs);
        let done = core.shutdown_barrier();
        assert!(done.load(Ordering::Acquire));
        assert_eq!(core.error.as_deref(), Some("Library worker is unavailable"));
    }

    #[test]
    fn warm_analysis_cache_preserves_waveform_summary_arc() {
        let root = std::env::temp_dir().join(format!(
            "cadence-controller-analysis-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after the epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create cache fixture directory");
        let path = root.join("source.wav");
        let cache_path = root.join("waveform.json");
        std::fs::write(&path, b"verified source bytes").expect("write source fixture");
        let verified = crate::source::open_and_hash(&path, || false).expect("hash source");
        let proof = verified.proof().clone();
        let ticket = verified.ticket();
        let waveform = audio::WaveformData {
            sample_rate: 48_000,
            channels: 1,
            duration_millis: 100,
            render_frames: 2,
            integrated_lufs: None,
            loudness_profile: Arc::from([]),
            summary: Arc::new(
                crate::signal_summary::GpuSignalSummary::from_interleaved_samples(
                    &[0.25, 0.0, -0.25, 0.0],
                    2,
                    2,
                ),
            ),
        };
        audio::write_waveform_cache_if_unchanged(&path, &cache_path, &ticket, &waveform)
            .expect("write persistent analysis");
        let decoded = audio::load_decoded_audio_cache(&path, &cache_path, &proof, || false)
            .expect("load persistent analysis")
            .expect("cache should hit");
        let mut cache = AnalysisCache::default();
        cache.insert(decoded.clone());

        let warm = cache
            .get(&path, Some(&proof), &|| false)
            .expect("warm lookup should succeed")
            .expect("warm lookup should hit");
        assert!(Arc::ptr_eq(
            &decoded.waveform().summary,
            &warm.waveform().summary
        ));
        std::fs::write(&path, b"replaced bytes").expect("replace source fixture");
        assert!(
            cache
                .get(&path, Some(&proof), &|| false)
                .expect("stale RAM entry should be discarded")
                .is_none()
        );
        assert!(cache.entries.is_empty());
        std::fs::remove_dir_all(root).expect("remove cache fixture directory");
    }

    #[test]
    fn saving_an_existing_note_updates_text_and_dragged_time_in_order() {
        let root = std::env::temp_dir().join(format!(
            "cadence-controller-note-retime-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after the epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create note fixture directory");
        let path = root.join("source.wav");
        let samples = vec![0_i16; 800];
        let data_len = (samples.len() * 2) as u32;
        let mut wav = Vec::from(*b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&8_000_u32.to_le_bytes());
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, wav).expect("write valid audio fixture");
        let decoded = audio::decode_audio_file(&path).expect("decode valid audio fixture");

        let (mut core, _, _) = isolated();
        core.library.tracks[0].path = path.clone();
        core.library.tracks[0].source_proof =
            SourceProvenance::Verified(decoded.source_proof().clone());
        core.library.tracks[0].notes = vec![
            Note {
                id: "first".into(),
                time_millis: 10,
                body: "Before".into(),
                done: false,
            },
            Note {
                id: "dragged".into(),
                time_millis: 40,
                body: "Old text".into(),
                done: false,
            },
            Note {
                id: "last".into(),
                time_millis: 80,
                body: "After".into(),
                done: false,
            },
        ]
        .into();
        core.decoded[0] = Some(decoded);

        assert!(core.save_note(Source::Main, Some("dragged"), 999, "Updated text"));
        let notes = core.notes(Source::Main);
        assert_eq!(
            notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "last", "dragged"]
        );
        assert_eq!(
            notes[2].time_millis, 100,
            "dragged time is clamped to duration"
        );
        assert_eq!(notes[2].body, "Updated text");
        std::fs::remove_dir_all(root).expect("remove note fixture directory");
    }

    #[test]
    fn imports_exclude_edits_and_selection_until_the_serialized_snapshot_returns() {
        let (mut core, jobs, _) = isolated();
        core.import(vec!["/new.wav".into()], ImportKind::Main);
        assert!(matches!(
            jobs.try_recv().unwrap(),
            Work::Import(_, _, ImportKind::Main)
        ));
        core.select("two".into());
        core.favorite("one");
        assert_eq!(core.library.selected_track_id.as_deref(), Some("one"));
        assert!(!core.library.tracks[0].favorite);
        assert_eq!(core.revision, 0);
        assert!(jobs.try_recv().is_err());
    }

    #[test]
    fn failed_save_prevents_import_and_a_later_save_ack_clears_the_dirty_barrier() {
        let (mut core, jobs, events) = isolated();
        core.favorite("one");
        assert!(matches!(jobs.try_recv().unwrap(), Work::Save(1, _)));
        events
            .send(Event::Saved(1, Err("disk full".into())))
            .unwrap();
        core.poll();
        assert!(core.dirty());
        core.import(vec!["/new.wav".into()], ImportKind::Main);
        assert!(!core.importing);
        assert!(jobs.try_recv().is_err());
        core.favorite("two");
        assert!(matches!(jobs.try_recv().unwrap(), Work::Save(2, _)));
        events
            .send(Event::Saved(2, Ok(storage::PersistenceOutcome::Durable)))
            .unwrap();
        core.poll();
        assert!(!core.dirty());
    }

    #[test]
    fn old_analysis_results_do_not_clear_a_new_selection_analysis() {
        let (mut core, _, events) = isolated();
        core.select("two".into());
        assert_eq!(core.generation, 5);
        core.error = None;
        events
            .send(Event::Decoded(4, Source::Main, Err("stale failure".into())))
            .unwrap();
        core.poll();
        assert!(core.analyzing[0]);
        assert!(core.error.is_none());
    }

    #[test]
    fn playhead_waits_for_the_seek_acknowledgement_and_ignores_old_generations() {
        let (mut core, _, _) = isolated();
        core.position = 3000;
        core.pending_tokens[0] = 9;
        core.transports[0].set_snapshot_for_test(Snapshot {
            generation: 4,
            acknowledged_token: 8,
            position_millis: 500,
            playing: true,
            ready: true,
        });
        core.poll();
        assert_eq!(core.position, 3000);
        assert!(!core.playing);
        core.transports[0].set_snapshot_for_test(Snapshot {
            generation: 4,
            acknowledged_token: 9,
            position_millis: 3100,
            playing: true,
            ready: true,
        });
        core.poll();
        assert_eq!(core.position, 3100);
        assert!(core.playing);
        core.transports[0].set_snapshot_for_test(Snapshot {
            generation: 3,
            acknowledged_token: 20,
            position_millis: 10,
            playing: false,
            ready: true,
        });
        core.poll();
        assert_eq!(core.position, 3100);
    }

    #[test]
    fn revision_switching_and_edits_preserve_per_build_notes_and_persist() {
        let root = std::env::temp_dir().join(format!(
            "cadence-controller-revisions-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let make_wav = |name: &str, amp: i16| {
            let path = root.join(name);
            let samples = vec![amp; 800];
            let data_len = (samples.len() * 2) as u32;
            let mut wav = Vec::from(*b"RIFF");
            wav.extend_from_slice(&(36 + data_len).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16_u32.to_le_bytes());
            wav.extend_from_slice(&1_u16.to_le_bytes());
            wav.extend_from_slice(&1_u16.to_le_bytes());
            wav.extend_from_slice(&8_000_u32.to_le_bytes());
            wav.extend_from_slice(&16_000_u32.to_le_bytes());
            wav.extend_from_slice(&2_u16.to_le_bytes());
            wav.extend_from_slice(&16_u16.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&data_len.to_le_bytes());
            for sample in samples {
                wav.extend_from_slice(&sample.to_le_bytes());
            }
            std::fs::write(&path, wav).unwrap();
            let decoded = audio::decode_audio_file(&path).unwrap();
            (path, decoded)
        };
        let (b1_path, b1_decoded) = make_wav("A1_b1.wav", 1000);
        let (b2_path, b2_decoded) = make_wav("A1_b2.wav", 2000);

        let (mut core, jobs, _) = isolated();
        core.library.tracks[0].group_key = Some("A1".into());
        core.library.tracks[0].path = b2_path.clone();
        core.library.tracks[0].original_name = "A1_b2.wav".into();
        core.library.tracks[0].source_proof =
            SourceProvenance::Verified(b2_decoded.source_proof().clone());
        core.library.tracks[0].revisions = vec![
            storage::BounceRevision {
                id: "rev-2".into(),
                build: Some(2),
                label: "b2".into(),
                original_name: "A1_b2.wav".into(),
                path: b2_path.clone(),
                source_proof: SourceProvenance::Verified(b2_decoded.source_proof().clone()),
                size: 1644,
                notes: Default::default(),
                feedback_note: None,
            },
            storage::BounceRevision {
                id: "rev-1".into(),
                build: Some(1),
                label: "b1".into(),
                original_name: "A1_b1.wav".into(),
                path: b1_path.clone(),
                source_proof: SourceProvenance::Verified(b1_decoded.source_proof().clone()),
                size: 1644,
                notes: vec![Note {
                    id: "b1-note".into(),
                    time_millis: 30,
                    body: "Dylan note on b1".into(),
                    done: false,
                }]
                .into(),
                feedback_note: Some("Heard by Dylan".into()),
            },
        ]
        .into();
        core.library.tracks[0].current_revision_id = Some("rev-2".into());
        core.library.tracks[0].active_revision_id = Some("rev-2".into());
        core.decoded[0] = Some(b2_decoded);

        assert!(core.save_note(Source::Main, None, 60, "Mix tweak on b2"));
        assert_eq!(core.library.tracks[0].revisions[0].notes.len(), 1);
        assert!(core.set_next_action("one", "Check sub on club system"));
        assert!(core.set_title("one", "Renamed Without Split"));
        assert!(core.set_revision_feedback("one", Some("rev-2"), "Ready to test"));

        // Switch to b1: loads b1 path and shows b1's timestamped notes.
        core.select_revision("one", "rev-1");
        assert_eq!(core.library.tracks[0].path, b1_path);
        assert_eq!(core.library.tracks[0].notes.len(), 1);
        assert_eq!(core.library.tracks[0].notes[0].body, "Dylan note on b1");
        assert_eq!(core.library.tracks[0].notes[0].time_millis, 30);
        assert!(jobs.try_recv().is_ok());

        // Missing source sets recovery flag without dropping notes.
        std::fs::remove_file(&b1_path).unwrap();
        core.load_selected();
        assert!(core.missing_source[0]);
        assert_eq!(core.library.tracks[0].notes.len(), 1);

        std::fs::remove_dir_all(root).unwrap();
    }
}
