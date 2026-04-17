#![allow(dead_code)]
use simplex::include_simf;
use simplex::program::{ArgumentsTrait, Program};

include_simf!("simf/timestamp_covenant.simf");

pub struct TimestampCovenantProgram {
    program: Program,
}

impl TimestampCovenantProgram {
    pub const SOURCE: &'static str = derived_timestamp_covenant::TIMESTAMP_COVENANT_CONTRACT_SOURCE;
    pub fn new(arguments: impl ArgumentsTrait + 'static) -> Self {
        Self {
            program: Program::new(Self::SOURCE, Box::new(arguments)),
        }
    }
    pub fn get_program(&self) -> &Program {
        &self.program
    }
    pub fn get_program_mut(&mut self) -> &mut Program {
        &mut self.program
    }
}
