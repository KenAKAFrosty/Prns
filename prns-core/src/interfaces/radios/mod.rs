//! Chip-scoped radio drivers, shared by the modulation-level interface families above
//! them: [`sx126x`] carries LoRa today and holds the GFSK seam for the modulations to
//! come, all over the one command set the SX126x family speaks. Future chips land here
//! as siblings.

pub mod sx126x;
