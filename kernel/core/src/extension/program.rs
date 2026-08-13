// SPDX-License-Identifier: MPL-2.0

//! Program loading and static verification.

use alloc::{boxed::Box, vec, vec::Vec};

use super::isa::{
    self, AccessWidth, DecodeError, HelperId, Instruction, REGISTER_COUNT, SCRATCH_SIZE,
};

pub(super) const ISA_VERSION: u16 = 1;
pub(super) const SYSCALL_OBSERVER_INTERFACE: u16 = 1;
pub(super) const SYSCALL_OBSERVER_VERSION: u16 = 1;
const MAX_EVENT_PAYLOAD_SIZE: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProgramMetadata {
    isa_version: u16,
    interface: u16,
    interface_version: u16,
}

impl ProgramMetadata {
    #[cfg_attr(
        not(ktest),
        expect(dead_code, reason = "reserved for the future user-facing loader")
    )]
    pub(super) const fn new(
        isa_version: u16,
        interface: u16,
        interface_version: u16,
    ) -> Self {
        Self {
            isa_version,
            interface,
            interface_version,
        }
    }

    #[cfg(ktest)]
    pub(super) const fn syscall_observer() -> Self {
        Self {
            isa_version: ISA_VERSION,
            interface: SYSCALL_OBSERVER_INTERFACE,
            interface_version: SYSCALL_OBSERVER_VERSION,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VerifyError {
    Decode(DecodeError),
    UnsupportedIsaVersion,
    UnsupportedInterface,
    UnsupportedInterfaceVersion,
    InvalidRegister { instruction: usize, register: u8 },
    UninitializedRegister { instruction: usize, register: u8 },
    ScratchOutOfBounds { instruction: usize },
    UninitializedScratch { instruction: usize },
    InvalidJump { instruction: usize, target: u32 },
    UnreachableInstruction { instruction: usize },
    PathWithoutExit { instruction: usize },
    HelperArgumentMustBeConstant { instruction: usize },
    InvalidHelperArguments { instruction: usize },
}

impl From<DecodeError> for VerifyError {
    fn from(value: DecodeError) -> Self {
        Self::Decode(value)
    }
}

pub(super) struct Program {
    instructions: Box<[Instruction]>,
}

impl Program {
    pub(super) fn load(bytes: &[u8], metadata: ProgramMetadata) -> Result<Self, VerifyError> {
        check_metadata(metadata)?;
        let instructions = isa::decode(bytes)?;
        verify(&instructions)?;
        Ok(Self {
            instructions: instructions.into_boxed_slice(),
        })
    }

    pub(super) fn instructions(&self) -> &[Instruction] {
        &self.instructions
    }
}

fn check_metadata(metadata: ProgramMetadata) -> Result<(), VerifyError> {
    if metadata.isa_version != ISA_VERSION {
        return Err(VerifyError::UnsupportedIsaVersion);
    }
    if metadata.interface != SYSCALL_OBSERVER_INTERFACE {
        return Err(VerifyError::UnsupportedInterface);
    }
    if metadata.interface_version != SYSCALL_OBSERVER_VERSION {
        return Err(VerifyError::UnsupportedInterfaceVersion);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegisterState {
    Uninitialized,
    Scalar,
    Constant(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AbstractState {
    registers: [RegisterState; REGISTER_COUNT],
    initialized_scratch: [u64; SCRATCH_SIZE / 64],
}

impl AbstractState {
    const fn initial() -> Self {
        Self {
            registers: [RegisterState::Uninitialized; REGISTER_COUNT],
            initialized_scratch: [0; SCRATCH_SIZE / 64],
        }
    }

    fn merge(self, other: Self) -> Self {
        let mut merged = self;
        for (dst, rhs) in merged.registers.iter_mut().zip(other.registers) {
            *dst = match (*dst, rhs) {
                (RegisterState::Constant(lhs), RegisterState::Constant(rhs)) if lhs == rhs => {
                    RegisterState::Constant(lhs)
                }
                (RegisterState::Uninitialized, _) | (_, RegisterState::Uninitialized) => {
                    RegisterState::Uninitialized
                }
                _ => RegisterState::Scalar,
            };
        }
        for (dst, rhs) in merged
            .initialized_scratch
            .iter_mut()
            .zip(other.initialized_scratch)
        {
            *dst &= rhs;
        }
        merged
    }

    fn register(&self, instruction: usize, register: u8) -> Result<RegisterState, VerifyError> {
        let state = *self.registers.get(register as usize).ok_or(
            VerifyError::InvalidRegister {
                instruction,
                register,
            },
        )?;
        if state == RegisterState::Uninitialized {
            return Err(VerifyError::UninitializedRegister {
                instruction,
                register,
            });
        }
        Ok(state)
    }

    fn set_register(
        &mut self,
        instruction: usize,
        register: u8,
        state: RegisterState,
    ) -> Result<(), VerifyError> {
        let slot = self.registers.get_mut(register as usize).ok_or(
            VerifyError::InvalidRegister {
                instruction,
                register,
            },
        )?;
        *slot = state;
        Ok(())
    }

    fn scratch_range(
        instruction: usize,
        offset: u32,
        width: AccessWidth,
    ) -> Result<core::ops::Range<usize>, VerifyError> {
        let start = offset as usize;
        let end = start
            .checked_add(width.bytes())
            .filter(|end| *end <= SCRATCH_SIZE)
            .ok_or(VerifyError::ScratchOutOfBounds { instruction })?;
        Ok(start..end)
    }

    fn mark_scratch(&mut self, range: core::ops::Range<usize>) {
        for byte in range {
            self.initialized_scratch[byte / 64] |= 1 << (byte % 64);
        }
    }

    fn check_scratch(
        &self,
        instruction: usize,
        range: core::ops::Range<usize>,
    ) -> Result<(), VerifyError> {
        if range
            .into_iter()
            .any(|byte| self.initialized_scratch[byte / 64] & (1 << (byte % 64)) == 0)
        {
            return Err(VerifyError::UninitializedScratch { instruction });
        }
        Ok(())
    }
}

fn verify(instructions: &[Instruction]) -> Result<(), VerifyError> {
    let mut incoming: Vec<Option<AbstractState>> = vec![None; instructions.len()];
    incoming[0] = Some(AbstractState::initial());

    for (index, instruction) in instructions.iter().copied().enumerate() {
        let mut state = incoming[index]
            .ok_or(VerifyError::UnreachableInstruction { instruction: index })?;
        let mut jump_target = None;
        let mut falls_through = true;

        match instruction {
            Instruction::LoadContext { dst, .. } => {
                state.set_register(index, dst, RegisterState::Scalar)?;
            }
            Instruction::LoadScratch { dst, offset, width } => {
                let range = AbstractState::scratch_range(index, offset, width)?;
                state.check_scratch(index, range)?;
                state.set_register(index, dst, RegisterState::Scalar)?;
            }
            Instruction::StoreScratch { offset, src, width } => {
                state.register(index, src)?;
                let range = AbstractState::scratch_range(index, offset, width)?;
                state.mark_scratch(range);
            }
            Instruction::MovImm { dst, value } => {
                state.set_register(index, dst, RegisterState::Constant(value))?;
            }
            Instruction::Add { dst, lhs, rhs } => {
                let lhs = state.register(index, lhs)?;
                let rhs = state.register(index, rhs)?;
                let result = match (lhs, rhs) {
                    (RegisterState::Constant(lhs), RegisterState::Constant(rhs)) => {
                        RegisterState::Constant(lhs.wrapping_add(rhs))
                    }
                    _ => RegisterState::Scalar,
                };
                state.set_register(index, dst, result)?;
            }
            Instruction::CompareEq { dst, lhs, rhs } => {
                state.register(index, lhs)?;
                state.register(index, rhs)?;
                state.set_register(index, dst, RegisterState::Scalar)?;
            }
            Instruction::JumpIfZero { condition, target } => {
                state.register(index, condition)?;
                let target_index = target as usize;
                if target_index <= index || target_index >= instructions.len() {
                    return Err(VerifyError::InvalidJump {
                        instruction: index,
                        target,
                    });
                }
                jump_target = Some(target_index);
            }
            Instruction::CallHelper { helper, arg0, arg1 } => {
                verify_helper(index, helper, arg0, arg1, &state)?;
                state.set_register(index, 0, RegisterState::Scalar)?;
            }
            Instruction::Exit => falls_through = false,
        }

        if let Some(target) = jump_target {
            propagate(&mut incoming[target], state);
        }
        if falls_through {
            let next = index + 1;
            if next == instructions.len() {
                return Err(VerifyError::PathWithoutExit { instruction: index });
            }
            propagate(&mut incoming[next], state);
        }
    }
    Ok(())
}

fn propagate(slot: &mut Option<AbstractState>, state: AbstractState) {
    *slot = Some(match *slot {
        Some(existing) => existing.merge(state),
        None => state,
    });
}

fn verify_helper(
    instruction: usize,
    helper: HelperId,
    arg0: u8,
    arg1: u8,
    state: &AbstractState,
) -> Result<(), VerifyError> {
    match helper {
        HelperId::ReadTime => {
            if arg0 != 0 || arg1 != 0 {
                return Err(VerifyError::InvalidHelperArguments { instruction });
            }
            Ok(())
        }
        HelperId::EmitEvent => {
            let (RegisterState::Constant(offset), RegisterState::Constant(length)) = (
                state.register(instruction, arg0)?,
                state.register(instruction, arg1)?,
            ) else {
                return Err(VerifyError::HelperArgumentMustBeConstant { instruction });
            };
            let start = usize::try_from(offset)
                .map_err(|_| VerifyError::InvalidHelperArguments { instruction })?;
            let length = usize::try_from(length)
                .map_err(|_| VerifyError::InvalidHelperArguments { instruction })?;
            let end = start
                .checked_add(length)
                .filter(|end| *end <= SCRATCH_SIZE && length <= MAX_EVENT_PAYLOAD_SIZE)
                .ok_or(VerifyError::InvalidHelperArguments { instruction })?;
            state.check_scratch(instruction, start..end)
        }
    }
}
