//! Optional operator-owned mission sequence. No file formats or wire fields.
use super::*;

pub struct Round {
    pub spec: MissionSpec,
    pub settings: Vec<(u8, u32)>,
    pub time_limit: Option<Duration>,
}
pub trait MissionSequence: Send {
    fn next(&mut self) -> Option<Result<Round, String>>;
    fn attempts(&self) -> usize;
    fn notice(&mut self) -> Option<String>;
    fn validate(&self, _spec: &MissionSpec, _world: &World) -> Result<(), String> {
        Ok(())
    }
    fn loaded(&mut self);
    fn failed(&mut self, why: &str);
}
/// Apply the settings that are part of the mission build, for trial builds too.
pub fn configured_spec(spec: MissionSpec, settings: &crate::settings::Store) -> MissionSpec {
    king::with_settings(spec, settings)
}
impl Host {
    /// Trial-build an operator candidate with the normal loading keepalives.
    /// Nothing is installed until the next mission boundary.
    pub fn check_sequence_round(
        &mut self,
        round: Round,
        validate: impl FnOnce(&MissionSpec, &World) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut config = self.config.clone();
        config.settings = round.settings;
        config.time_limit = round.time_limit;
        config.validate().map_err(|e| e.to_string())?;
        let settings = crate::settings::Store::from_config(&config);
        let mut spec = king::with_settings(round.spec, &settings);
        let (world, _) = self.build_seeded(&mut spec).map_err(|e| e.to_string())?;
        validate(&spec, &world)
    }

    pub fn set_mission_sequence(&mut self, sequence: Box<dyn MissionSequence>) {
        self.config.after_end = AfterEnd::Restart;
        self.sequence = Some(sequence);
    }
    pub(super) fn sequence_notice(&mut self) {
        if let Some(text) = self.sequence.as_mut().and_then(|s| s.notice()) {
            let ids: Vec<_> = self.peers.keys().copied().collect();
            for id in ids {
                self.send(id, &Message::Notice(text.clone()));
            }
        }
    }
    /// Try each candidate at most once. Install only a successfully built round.
    pub(super) fn sequence_mission(&mut self) -> bool {
        let Some(mut sequence) = self.sequence.take() else {
            return false;
        };
        let mut loaded = false;
        let mut exhausted = true;
        for _ in 0..sequence.attempts() {
            let Some(round) = sequence.next() else {
                exhausted = false;
                break;
            };
            let result = round.and_then(|round| {
                let mut config = self.config.clone();
                config.settings = round.settings;
                config.time_limit = round.time_limit;
                config.validate().map_err(|e| e.to_string())?;
                let settings = crate::settings::Store::from_config(&config);
                let spec = king::with_settings(round.spec, &settings);
                let mut spec =
                    with_surface_seed(spec, self.session_id, self.number.wrapping_add(1));
                let (world, manifest) = self.build_seeded(&mut spec).map_err(|e| e.to_string())?;
                sequence.validate(&spec, &world)?;
                self.config = config;
                self.settings = settings;
                self.spec = spec;
                self.spec_text = self.spec.to_text();
                self.world = world;
                self.manifest = manifest;
                self.number = self.number.wrapping_add(1);
                self.court = king::Court::new(&self.config, &self.spec);
                for peer in self.peers.values_mut() {
                    peer.lobby.release();
                    peer.lobby.ready = false;
                    peer.lobby.unable = None;
                }
                self.tracker = Tracker::new(&self.world);
                self.revival = revive::Revivals::default();
                self.gives.clear();
                self.origin = None;
                self.ticks_run = 0;
                self.ever_seated = false;
                self.empty_since = None;
                self.lobby_dirty = true;
                self.life = Life::Lobby;
                let mission = self.mission_message();
                let ids: Vec<_> = self.peers.keys().copied().collect();
                for id in ids {
                    self.send(id, &mission);
                }
                self.roster_dirty = true;
                self.log(HostLog::MissionRestarted { tick: 0 });
                // The new round's own minimum (and AI rule) apply from here.
                self.note_start_wait(true);
                Ok(())
            });
            match result {
                Ok(()) => {
                    sequence.loaded();
                    loaded = true;
                    break;
                }
                Err(why) => sequence.failed(&why),
            }
        }
        self.sequence = Some(sequence);
        if !loaded {
            if exhausted {
                self.log(HostLog::Fault {
                    tick: self.world.tick(),
                    text: "Every rotation entry failed. Stopping.".into(),
                });
            }
            if self.peers.is_empty() {
                self.stop();
            } else {
                // Let already queued results/debriefs be acknowledged, including
                // a zero-delay final round, before closing the transport.
                let deadline = self.now + CLOSE_GRACE;
                for peer in self.peers.values_mut() {
                    peer.ended = true;
                    peer.stage = Stage::Closing {
                        deadline,
                        reason: DisconnectReason::ServerStopping,
                    };
                }
                self.life = Life::Ended {
                    next_at: None,
                    stop_at: deadline,
                };
            }
        } else if self.start_rule() == StartMode::Now {
            let _ = self.start_flying();
        }
        true
    }
}
