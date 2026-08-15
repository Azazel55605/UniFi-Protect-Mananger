//! Archiving: packing aged-out clips into per-camera monthly tars, proving the
//! copy, and only then removing the originals.

pub mod pack;
pub mod plan;
pub mod run;
pub mod schedule;
