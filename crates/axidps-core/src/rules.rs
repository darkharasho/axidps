//! WvW log rules (plan "Builder choices" item 3).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Only squad members start a log and count as activity.
    Squad,
    /// Dev flag: any agent counts as squad, until identity exists.
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WvwRules {
    pub trigger: Trigger,
    /// Save only if at least this many squad members were seen during the log.
    pub min_squad: usize,
    /// Save only if the log lasts at least this long.
    pub min_duration_ms: u64,
    /// A log ends after this long without squad activity.
    pub inactive_ms: u64,
    /// Write the condition model's estimated ticks for other players. Off: the model only runs
    /// its self-check against the POV's exact ticks (design §4) until a session validates it.
    pub condi_write: bool,
}

impl Default for WvwRules {
    fn default() -> Self {
        Self {
            trigger: Trigger::Squad,
            min_squad: 5,
            min_duration_ms: 10_000,
            inactive_ms: 20_000,
            condi_write: false,
        }
    }
}
