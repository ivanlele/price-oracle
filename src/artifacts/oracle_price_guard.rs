#![allow(dead_code)]
use simplex::include_simf;
use simplex::program::{ArgumentsTrait, Program};

include_simf!("simf/oracle_price_guard.simf");

pub struct OraclePriceGuardProgram {
    program: Program,
}

impl OraclePriceGuardProgram {
    pub const SOURCE: &'static str = derived_oracle_price_guard::ORACLE_PRICE_GUARD_CONTRACT_SOURCE;

    pub fn new(arguments: impl ArgumentsTrait + 'static) -> Self {
        Self {
            program: Program::new(Self::SOURCE, Box::new(arguments)),
        }
    }

    pub fn get_program(&self) -> &Program {
        &self.program
    }
}
