#![no_std]

//! Pure port vocabulary: types and a trait, no dependencies, no effects.

pub struct AgentId(pub u32);

pub trait Port {
    fn get(&self) -> u8;
}
