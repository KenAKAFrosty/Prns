#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropRouteOutcome {
    Dropped,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropRoutesViaOutcome {
    pub dropped_routes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClearAnnounceQueuesOutcome {
    pub dropped_announces: u32,
}
