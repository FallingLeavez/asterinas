// SPDX-License-Identifier: MPL-2.0

//! Safe-Rust interpreter for verified extension programs.

use super::{
    isa::{ContextField, HelperId, Instruction, MAX_INSTRUCTIONS, REGISTER_COUNT, SCRATCH_SIZE},
    program::Program,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SyscallContext {
    pub syscall_number: u64,
    pub process_id: u64,
    pub thread_id: u64,
    pub arguments: [u64; 6],
    pub timestamp: u64,
}

impl SyscallContext {
    fn read(self, field: ContextField) -> u64 {
        match field {
            ContextField::SyscallNumber => self.syscall_number,
            ContextField::ProcessId => self.process_id,
            ContextField::ThreadId => self.thread_id,
            ContextField::Argument(index) => self.arguments[index as usize],
            ContextField::Timestamp => self.timestamp,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RuntimeError {
    InstructionBudgetExceeded,
    ScratchOutOfBounds,
}

pub(super) trait HelperDispatcher {
    fn call(
        &self,
        helper: HelperId,
        arg0: u64,
        arg1: u64,
        scratch: &[u8; SCRATCH_SIZE],
        context: &SyscallContext,
    ) -> Result<u64, RuntimeError>;
}

pub(super) fn run(
    program: &Program,
    context: &SyscallContext,
    helpers: &impl HelperDispatcher,
) -> Result<(), RuntimeError> {
    let mut registers = [0u64; REGISTER_COUNT];
    let mut scratch = [0u8; SCRATCH_SIZE];
    let mut pc = 0usize;
    let mut remaining_budget = MAX_INSTRUCTIONS;

    loop {
        if remaining_budget == 0 {
            return Err(RuntimeError::InstructionBudgetExceeded);
        }
        remaining_budget -= 1;

        match program.instructions()[pc] {
            Instruction::LoadContext { dst, field } => {
                registers[dst as usize] = context.read(field);
            }
            Instruction::LoadScratch { dst, offset, width } => {
                let range = checked_range(offset as u64, width.bytes() as u64)?;
                let mut raw = [0u8; 8];
                raw[..range.len()].copy_from_slice(&scratch[range]);
                registers[dst as usize] = u64::from_le_bytes(raw);
            }
            Instruction::StoreScratch { offset, src, width } => {
                let range = checked_range(offset as u64, width.bytes() as u64)?;
                let raw = registers[src as usize].to_le_bytes();
                scratch[range.clone()].copy_from_slice(&raw[..range.len()]);
            }
            Instruction::MovImm { dst, value } => registers[dst as usize] = value,
            Instruction::Add { dst, lhs, rhs } => {
                registers[dst as usize] =
                    registers[lhs as usize].wrapping_add(registers[rhs as usize]);
            }
            Instruction::CompareEq { dst, lhs, rhs } => {
                registers[dst as usize] =
                    u64::from(registers[lhs as usize] == registers[rhs as usize]);
            }
            Instruction::JumpIfZero { condition, target } => {
                if registers[condition as usize] == 0 {
                    pc = target as usize;
                    continue;
                }
            }
            Instruction::CallHelper { helper, arg0, arg1 } => {
                registers[0] = helpers.call(
                    helper,
                    registers[arg0 as usize],
                    registers[arg1 as usize],
                    &scratch,
                    context,
                )?;
            }
            Instruction::Exit => return Ok(()),
        }
        pc += 1;
    }
}

pub(super) fn checked_range(
    offset: u64,
    length: u64,
) -> Result<core::ops::Range<usize>, RuntimeError> {
    let start = usize::try_from(offset).map_err(|_| RuntimeError::ScratchOutOfBounds)?;
    let length = usize::try_from(length).map_err(|_| RuntimeError::ScratchOutOfBounds)?;
    let end = start
        .checked_add(length)
        .filter(|end| *end <= SCRATCH_SIZE)
        .ok_or(RuntimeError::ScratchOutOfBounds)?;
    Ok(start..end)
}
