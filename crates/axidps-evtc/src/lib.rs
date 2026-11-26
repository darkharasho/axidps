//! arcdps-compatible EVTC (revision 1) log model and encoder.

mod container;
mod encode;
mod file;
mod model;

pub use container::{unzevtc, zevtc};
pub use encode::{AGENT_SIZE, EVENT_SIZE, HEADER_SIZE, SKILL_SIZE, encode};
pub use file::{default_log_dir, log_file_stem, write_log_atomic};
pub use model::{Agent, BOSS_ID_WVW, Event, FORMAT_BUILD, Header, Log, Skill, player_name};
