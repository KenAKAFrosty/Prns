mod learning;
mod lifetime;
mod lookup;
mod model;
mod persistence;
mod removal;
mod updates;

pub use model::RoutingTable;

#[cfg(test)]
mod tests;
