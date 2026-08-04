//! Host-controlled audition playback for the native Cadence review surface.
//!
//! The Radiant reducer only sends small, generation-tagged commands and reads a
//! non-blocking snapshot. Output setup, decoder construction, and transport
//! control are owned by this host module. Rodio/CPAL may still pull decoder
//! data and service internal control state from the output callback, so this is
//! intentionally not a lock-free realtime or sample-accurate audio engine.

use rodio::{Decoder, DeviceSinkBuilder, Player};
use std::{
    fs::File,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::Duration,
};

const COMMAND_CAPACITY: usize = 32;
const CONTROL_INTERVAL: Duration = Duration::from_millis(8);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u64,
    pub acknowledged_token: u64,
    pub position_millis: u64,
    pub playing: bool,
    pub ready: bool,
}

#[derive(Debug)]
struct SharedSnapshot {
    generation: AtomicU64,
    requested_generation: AtomicU64,
    acknowledged_token: AtomicU64,
    position_millis: AtomicU64,
    playing: AtomicBool,
    ready: AtomicBool,
    error_available: AtomicBool,
    error: Mutex<Option<(u64, String)>>,
}

impl SharedSnapshot {
    fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            requested_generation: AtomicU64::new(0),
            acknowledged_token: AtomicU64::new(0),
            position_millis: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            error_available: AtomicBool::new(false),
            error: Mutex::new(None),
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            generation: self.generation.load(Ordering::Acquire),
            acknowledged_token: self.acknowledged_token.load(Ordering::Acquire),
            position_millis: self.position_millis.load(Ordering::Acquire),
            playing: self.playing.load(Ordering::Acquire),
            ready: self.ready.load(Ordering::Acquire),
        }
    }

    fn set_error(&self, generation: u64, error: String) {
        if let Ok(mut slot) = self.error.lock() {
            *slot = Some((generation, error));
            self.error_available.store(true, Ordering::Release);
        }
    }

    fn acknowledge(&self, token: u64) {
        let mut observed = self.acknowledged_token.load(Ordering::Acquire);
        while token > observed {
            match self.acknowledged_token.compare_exchange_weak(
                observed,
                token,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => observed = current,
            }
        }
    }

    fn take_error(&self, generation: u64) -> Option<String> {
        if !self.error_available.load(Ordering::Acquire) {
            return None;
        }
        let mut slot = self.error.try_lock().ok()?;
        match slot.take() {
            Some((error_generation, error)) if error_generation == generation => {
                self.error_available.store(false, Ordering::Release);
                Some(error)
            }
            Some(_) | None => {
                self.error_available.store(false, Ordering::Release);
                None
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Command {
    Load {
        token: u64,
        generation: u64,
        path: PathBuf,
        duration_millis: u64,
    },
    Unload {
        token: u64,
        generation: u64,
    },
    Play {
        token: u64,
        generation: u64,
    },
    Pause {
        token: u64,
        generation: u64,
    },
    Seek {
        token: u64,
        generation: u64,
        position_millis: u64,
        resume: bool,
    },
}

impl Command {
    fn load_generation(&self) -> Option<u64> {
        match self {
            Self::Load { generation, .. } => Some(*generation),
            Self::Unload { .. } | Self::Play { .. } | Self::Pause { .. } | Self::Seek { .. } => {
                None
            }
        }
    }
}

/// Single-slot, latest-wins admission for a load command when the bounded
/// control queue is full. Atomic pointer ownership keeps the UI path
/// non-blocking; only one heap allocation is retained at a time.
#[derive(Debug)]
struct PendingLoad {
    pointer: std::sync::atomic::AtomicPtr<Command>,
}

// SAFETY: Command is Send, and ownership of each boxed command moves through
// the atomic pointer exactly once via swap before it is reclaimed.
unsafe impl Send for PendingLoad {}
unsafe impl Sync for PendingLoad {}

impl PendingLoad {
    fn new() -> Self {
        Self {
            pointer: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    fn replace(&self, command: Command) {
        let replacement = Box::into_raw(Box::new(command));
        let previous = self.pointer.swap(replacement, Ordering::AcqRel);
        if !previous.is_null() {
            // SAFETY: the swap transfers exclusive ownership of the previous
            // allocation to this thread.
            unsafe { drop(Box::from_raw(previous)) };
        }
    }

    fn take(&self) -> Option<Command> {
        let pointer = self.pointer.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if pointer.is_null() {
            None
        } else {
            // SAFETY: the swap transfers exclusive ownership of this
            // allocation to this thread.
            Some(unsafe { *Box::from_raw(pointer) })
        }
    }

    fn clear_generation(&self, generation: u64) {
        let Some(command) = self.take() else {
            return;
        };
        if command.load_generation() != Some(generation) {
            self.replace(command);
        }
    }
}

impl Drop for PendingLoad {
    fn drop(&mut self) {
        let pointer = self.pointer.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !pointer.is_null() {
            // SAFETY: Drop has exclusive access to the slot.
            unsafe { drop(Box::from_raw(pointer)) };
        }
    }
}

#[derive(Clone, Debug)]
pub struct AudioTransport {
    commands: SyncSender<Command>,
    shared: Arc<SharedSnapshot>,
    pending_load: Arc<PendingLoad>,
    next_token: Arc<AtomicU64>,
}

impl AudioTransport {
    pub fn spawn() -> Self {
        let (commands, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let shared = Arc::new(SharedSnapshot::new());
        let pending_load = Arc::new(PendingLoad::new());
        let thread_shared = Arc::clone(&shared);
        let thread_pending_load = Arc::clone(&pending_load);
        thread::Builder::new()
            .name(String::from("cadence-audio-transport"))
            .spawn(move || run_transport(receiver, thread_shared, thread_pending_load))
            .expect("Cadence audio transport thread should spawn");
        Self {
            commands,
            shared,
            pending_load,
            next_token: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot()
    }

    pub fn take_error(&self, generation: u64) -> Option<String> {
        self.shared.take_error(generation)
    }

    pub fn load(
        &self,
        generation: u64,
        path: PathBuf,
        duration_millis: u64,
    ) -> Result<u64, String> {
        self.shared
            .requested_generation
            .store(generation, Ordering::Release);
        let token = self.next_token();
        let command = Command::Load {
            token,
            generation,
            path,
            duration_millis,
        };
        match self.commands.try_send(command) {
            Ok(()) => {
                self.clear_pending_load(generation);
                Ok(token)
            }
            // The transport thread will pick up the latest load intent from
            // the coalescing slot on its next control tick.
            Err(TrySendError::Full(command)) => {
                self.store_pending_load(command)?;
                Ok(token)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.clear_pending_load(generation);
                Err(String::from("The audio transport is no longer available."))
            }
        }
    }

    pub fn unload(&self, generation: u64) -> Result<u64, String> {
        self.shared
            .requested_generation
            .store(generation, Ordering::Release);
        let token = self.next_token();
        self.try_send(Command::Unload { token, generation })
            .map(|()| token)
    }

    pub fn play(&self, generation: u64) -> Result<u64, String> {
        let token = self.next_token();
        self.try_send(Command::Play { token, generation })
            .map(|()| token)
    }

    pub fn pause(&self, generation: u64) -> Result<u64, String> {
        let token = self.next_token();
        self.try_send(Command::Pause { token, generation })
            .map(|()| token)
    }

    pub fn seek(
        &self,
        generation: u64,
        position_millis: u64,
        duration_millis: u64,
        resume: bool,
    ) -> Result<u64, String> {
        let token = self.next_token();
        self.try_send(Command::Seek {
            token,
            generation,
            position_millis: clamp_position(position_millis, duration_millis),
            resume,
        })
        .map(|()| token)
    }

    fn try_send(&self, command: Command) -> Result<(), String> {
        match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                Err(String::from("Audio controls are busy — try again shortly."))
            }
            Err(TrySendError::Disconnected(_)) => {
                Err(String::from("The audio transport is no longer available."))
            }
        }
    }

    fn next_token(&self) -> u64 {
        self.next_token.fetch_add(1, Ordering::Relaxed)
    }

    fn store_pending_load(&self, command: Command) -> Result<(), String> {
        self.pending_load.replace(command);
        Ok(())
    }

    fn clear_pending_load(&self, generation: u64) {
        self.pending_load.clear_generation(generation);
    }
}

#[derive(Clone, Debug)]
struct LoadedTrack {
    generation: u64,
    path: PathBuf,
    duration_millis: u64,
}

fn run_transport(
    receiver: Receiver<Command>,
    shared: Arc<SharedSnapshot>,
    pending_load: Arc<PendingLoad>,
) {
    let output = match DeviceSinkBuilder::open_default_sink() {
        Ok(output) => {
            let mut output = output;
            output.log_on_drop(false);
            Some(output)
        }
        Err(error) => {
            shared.set_error(
                0,
                format!("Could not open the default audio output: {error}"),
            );
            None
        }
    };
    let mut player: Option<Player> = None;
    let mut loaded: Option<LoadedTrack> = None;

    loop {
        if let Some(command) = take_pending_load(&pending_load) {
            handle_command(command, &shared, output.as_ref(), &mut player, &mut loaded);
        }
        match receiver.recv_timeout(CONTROL_INTERVAL) {
            Ok(command) => {
                handle_command(command, &shared, output.as_ref(), &mut player, &mut loaded)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        loop {
            match receiver.try_recv() {
                Ok(command) => {
                    handle_command(command, &shared, output.as_ref(), &mut player, &mut loaded)
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        reconcile_stale_track(&shared, &mut player, &mut loaded);
        publish_snapshot(&shared, player.as_ref(), loaded.as_ref());
    }

    drop(player);
    drop(loaded);
    drop(output);
}

fn take_pending_load(pending_load: &PendingLoad) -> Option<Command> {
    pending_load.take()
}

fn reconcile_stale_track(
    shared: &SharedSnapshot,
    player: &mut Option<Player>,
    loaded: &mut Option<LoadedTrack>,
) {
    let requested_generation = shared.requested_generation.load(Ordering::Acquire);
    if loaded
        .as_ref()
        .is_some_and(|track| track.generation != requested_generation)
    {
        *player = None;
        *loaded = None;
        shared
            .generation
            .store(requested_generation, Ordering::Release);
        shared.position_millis.store(0, Ordering::Release);
        shared.playing.store(false, Ordering::Release);
        shared.ready.store(false, Ordering::Release);
    }
}

fn handle_command(
    command: Command,
    shared: &SharedSnapshot,
    output: Option<&rodio::MixerDeviceSink>,
    player: &mut Option<Player>,
    loaded: &mut Option<LoadedTrack>,
) {
    let (token, acknowledged) = match command {
        Command::Load {
            token,
            generation,
            path,
            duration_millis,
        } => (
            token,
            load_track(
                generation,
                path,
                duration_millis,
                shared,
                output,
                player,
                loaded,
            ),
        ),
        Command::Unload { token, generation } => {
            if !is_current(shared, generation) {
                (token, false)
            } else {
                *player = None;
                *loaded = None;
                shared.generation.store(generation, Ordering::Release);
                shared.position_millis.store(0, Ordering::Release);
                shared.playing.store(false, Ordering::Release);
                shared.ready.store(false, Ordering::Release);
                (token, true)
            }
        }
        Command::Play { token, generation } => {
            if !is_current(shared, generation) {
                (token, false)
            } else {
                let reloaded = if let Some(track) = loaded.clone()
                    && player.as_ref().is_some_and(Player::empty)
                {
                    load_track(
                        track.generation,
                        track.path,
                        track.duration_millis,
                        shared,
                        output,
                        player,
                        loaded,
                    )
                } else {
                    true
                };
                if !reloaded {
                    (token, false)
                } else {
                    if let Some(player) = player.as_ref()
                        && loaded
                            .as_ref()
                            .is_some_and(|track| track.generation == generation)
                    {
                        player.play();
                        shared.playing.store(true, Ordering::Release);
                    }
                    (token, true)
                }
            }
        }
        Command::Pause { token, generation } => {
            if !is_current(shared, generation) {
                (token, false)
            } else {
                if let Some(player) = player.as_ref() {
                    player.pause();
                }
                shared.playing.store(false, Ordering::Release);
                (token, true)
            }
        }
        Command::Seek {
            token,
            generation,
            position_millis,
            resume,
        } => {
            if !is_current(shared, generation) {
                (token, false)
            } else {
                match loaded.clone() {
                    None => (token, true),
                    Some(track) if track.generation != generation => (token, false),
                    Some(track) => {
                        let reloaded = if player.as_ref().is_some_and(Player::empty) {
                            load_track(
                                track.generation,
                                track.path,
                                track.duration_millis,
                                shared,
                                output,
                                player,
                                loaded,
                            )
                        } else {
                            true
                        };
                        if !reloaded {
                            (token, false)
                        } else if let Some(player) = player.as_ref() {
                            if let Err(error) =
                                player.try_seek(Duration::from_millis(position_millis))
                            {
                                shared.set_error(
                                    generation,
                                    format!("Could not seek this audio file: {error}"),
                                );
                                (token, true)
                            } else {
                                if resume {
                                    player.play();
                                } else {
                                    player.pause();
                                }
                                shared
                                    .position_millis
                                    .store(position_millis, Ordering::Release);
                                shared.playing.store(resume, Ordering::Release);
                                (token, true)
                            }
                        } else {
                            (token, true)
                        }
                    }
                }
            }
        }
    };
    if acknowledged {
        shared.acknowledge(token);
    }
}

fn load_track(
    generation: u64,
    path: PathBuf,
    duration_millis: u64,
    shared: &SharedSnapshot,
    output: Option<&rodio::MixerDeviceSink>,
    player: &mut Option<Player>,
    loaded: &mut Option<LoadedTrack>,
) -> bool {
    if !is_current(shared, generation) {
        return false;
    }

    *player = None;
    *loaded = None;
    shared.generation.store(generation, Ordering::Release);
    shared.position_millis.store(0, Ordering::Release);
    shared.playing.store(false, Ordering::Release);
    shared.ready.store(false, Ordering::Release);

    let Some(output) = output else {
        shared.set_error(
            generation,
            String::from("Could not open the default audio output."),
        );
        return true;
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            shared.set_error(
                generation,
                format!("Could not open {} for playback: {error}", path.display()),
            );
            return true;
        }
    };
    let byte_len = file.metadata().ok().map(|metadata| metadata.len());
    let mut builder = Decoder::builder().with_data(file);
    if let Some(byte_len) = byte_len {
        builder = builder.with_byte_len(byte_len);
    }
    if let Some(hint) = path.extension().and_then(|extension| extension.to_str()) {
        builder = builder.with_hint(&hint.to_ascii_lowercase());
    }
    let decoder = match builder.build() {
        Ok(decoder) => decoder,
        Err(error) => {
            shared.set_error(
                generation,
                format!("Could not decode {} for playback: {error}", path.display()),
            );
            return true;
        }
    };
    if !is_current(shared, generation) {
        return false;
    }

    let player_handle = Player::connect_new(output.mixer());
    player_handle.append(decoder);
    player_handle.pause();
    *player = Some(player_handle);
    *loaded = Some(LoadedTrack {
        generation,
        path,
        duration_millis,
    });
    shared.ready.store(true, Ordering::Release);
    true
}

fn publish_snapshot(
    shared: &SharedSnapshot,
    player: Option<&Player>,
    loaded: Option<&LoadedTrack>,
) {
    let Some(loaded) = loaded else {
        return;
    };
    let Some(player) = player else {
        return;
    };
    let ended = player.empty();
    let position_millis = if ended {
        loaded.duration_millis
    } else {
        clamp_position(player.get_pos().as_millis() as u64, loaded.duration_millis)
    };
    shared
        .position_millis
        .store(position_millis, Ordering::Release);
    shared
        .playing
        .store(!ended && !player.is_paused(), Ordering::Release);
    shared.ready.store(true, Ordering::Release);
}

fn is_current(shared: &SharedSnapshot, generation: u64) -> bool {
    shared.requested_generation.load(Ordering::Acquire) == generation
}

pub fn clamp_position(position_millis: u64, duration_millis: u64) -> u64 {
    position_millis.min(duration_millis)
}

#[cfg(test)]
mod tests {
    use super::{Command, PendingLoad, SharedSnapshot, clamp_position, is_current};
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    #[test]
    fn seek_position_is_saturated_to_track_duration() {
        assert_eq!(clamp_position(2_000, 1_000), 1_000);
        assert_eq!(clamp_position(250, 1_000), 250);
        assert_eq!(clamp_position(250, 0), 0);
    }

    #[test]
    fn stale_generation_is_rejected() {
        let shared = SharedSnapshot::new();
        shared.requested_generation.store(8, Ordering::Release);
        assert!(is_current(&shared, 8));
        assert!(!is_current(&shared, 7));
    }

    #[test]
    fn default_snapshot_is_idle_and_not_ready() {
        let shared = SharedSnapshot::new();
        assert_eq!(
            shared.snapshot(),
            super::Snapshot {
                generation: 0,
                acknowledged_token: 0,
                position_millis: 0,
                playing: false,
                ready: false,
            }
        );
    }

    #[test]
    fn acknowledged_tokens_never_move_backwards() {
        let shared = SharedSnapshot::new();
        shared.acknowledge(9);
        shared.acknowledge(4);
        assert_eq!(shared.snapshot().acknowledged_token, 9);
    }

    #[test]
    fn pending_load_slot_keeps_the_latest_generation() {
        let pending = PendingLoad::new();
        pending.replace(Command::Load {
            token: 1,
            generation: 1,
            path: PathBuf::from("first.wav"),
            duration_millis: 1_000,
        });
        pending.replace(Command::Load {
            token: 2,
            generation: 2,
            path: PathBuf::from("second.wav"),
            duration_millis: 2_000,
        });
        assert_eq!(
            pending.take().and_then(|command| command.load_generation()),
            Some(2)
        );
    }
}
