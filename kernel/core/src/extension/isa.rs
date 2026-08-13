// SPDX-License-Identifier: MPL-2.0

//! The fixed-width instruction encoding used by the extension prototype.

use alloc::vec::Vec;

pub(super) const INSTRUCTION_SIZE: usize = 16;
pub(super) const REGISTER_COUNT: usize = 8;
pub(super) const SCRATCH_SIZE: usize = 256;
pub(super) const MAX_INSTRUCTIONS: usize = 64;

const OP_LOAD_CONTEXT: u8 = 0x01;
const OP_LOAD_SCRATCH: u8 = 0x02;
const OP_STORE_SCRATCH: u8 = 0x03;
const OP_MOV_IMM: u8 = 0x04;
const OP_ADD: u8 = 0x05;
const OP_COMPARE_EQ: u8 = 0x06;
const OP_JUMP_IF_ZERO: u8 = 0x07;
const OP_CALL_HELPER: u8 = 0x08;
const OP_EXIT: u8 = 0x09;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContextField {
    SyscallNumber,
    ProcessId,
    ThreadId,
    Argument(u8),
    Timestamp,
}

impl ContextField {
    fn decode(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::SyscallNumber),
            1 => Some(Self::ProcessId),
            2 => Some(Self::ThreadId),
            3..=8 => Some(Self::Argument(value - 3)),
            9 => Some(Self::Timestamp),
            _ => None,
        }
    }

    #[cfg(ktest)]
    pub(super) const fn encode(self) -> u8 {
        match self {
            Self::SyscallNumber => 0,
            Self::ProcessId => 1,
            Self::ThreadId => 2,
            Self::Argument(index) => 3 + index,
            Self::Timestamp => 9,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AccessWidth {
    Byte,
    Half,
    Word,
    Double,
}

impl AccessWidth {
    fn decode(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Byte),
            2 => Some(Self::Half),
            4 => Some(Self::Word),
            8 => Some(Self::Double),
            _ => None,
        }
    }

    pub(super) const fn bytes(self) -> usize {
        match self {
            Self::Byte => 1,
            Self::Half => 2,
            Self::Word => 4,
            Self::Double => 8,
        }
    }

    #[cfg(ktest)]
    pub(super) const fn encode(self) -> u8 {
        self.bytes() as u8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HelperId {
    ReadTime,
    EmitEvent,
}

impl HelperId {
    fn decode(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::ReadTime),
            1 => Some(Self::EmitEvent),
            _ => None,
        }
    }

    #[cfg(ktest)]
    pub(super) const fn encode(self) -> u8 {
        match self {
            Self::ReadTime => 0,
            Self::EmitEvent => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Instruction {
    LoadContext {
        dst: u8,
        field: ContextField,
    },
    LoadScratch {
        dst: u8,
        offset: u32,
        width: AccessWidth,
    },
    StoreScratch {
        offset: u32,
        src: u8,
        width: AccessWidth,
    },
    MovImm {
        dst: u8,
        value: u64,
    },
    Add {
        dst: u8,
        lhs: u8,
        rhs: u8,
    },
    CompareEq {
        dst: u8,
        lhs: u8,
        rhs: u8,
    },
    JumpIfZero {
        condition: u8,
        target: u32,
    },
    CallHelper {
        helper: HelperId,
        arg0: u8,
        arg1: u8,
    },
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DecodeError {
    EmptyProgram,
    MisalignedLength,
    ProgramTooLarge,
    InvalidOpcode { instruction: usize, opcode: u8 },
    InvalidContextField { instruction: usize, field: u8 },
    InvalidAccessWidth { instruction: usize, width: u8 },
    InvalidHelper { instruction: usize, helper: u8 },
    NonCanonicalEncoding { instruction: usize },
}

pub(super) fn decode(bytes: &[u8]) -> Result<Vec<Instruction>, DecodeError> {
    if bytes.is_empty() {
        return Err(DecodeError::EmptyProgram);
    }
    if !bytes.len().is_multiple_of(INSTRUCTION_SIZE) {
        return Err(DecodeError::MisalignedLength);
    }
    if bytes.len() / INSTRUCTION_SIZE > MAX_INSTRUCTIONS {
        return Err(DecodeError::ProgramTooLarge);
    }

    bytes
        .chunks_exact(INSTRUCTION_SIZE)
        .enumerate()
        .map(|(index, raw)| decode_instruction(index, raw))
        .collect()
}

fn decode_instruction(index: usize, raw: &[u8]) -> Result<Instruction, DecodeError> {
    let opcode = raw[0];
    let a = raw[1];
    let b = raw[2];
    let c = raw[3];
    let operand = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    let immediate = u64::from_le_bytes([
        raw[8], raw[9], raw[10], raw[11], raw[12], raw[13], raw[14], raw[15],
    ]);

    let reserved_fields_are_nonzero_fn =
        |used_a: bool, used_b: bool, used_c: bool, used_operand: bool, used_imm: bool| {
        (!used_a && a != 0)
            || (!used_b && b != 0)
            || (!used_c && c != 0)
            || (!used_operand && operand != 0)
            || (!used_imm && immediate != 0)
        };

    let instruction = match opcode {
        OP_LOAD_CONTEXT => {
            if reserved_fields_are_nonzero_fn(true, true, false, false, false) {
                return Err(DecodeError::NonCanonicalEncoding { instruction: index });
            }
            let field = ContextField::decode(b).ok_or(DecodeError::InvalidContextField {
                instruction: index,
                field: b,
            })?;
            Instruction::LoadContext { dst: a, field }
        }
        OP_LOAD_SCRATCH => {
            if reserved_fields_are_nonzero_fn(true, true, false, true, false) {
                return Err(DecodeError::NonCanonicalEncoding { instruction: index });
            }
            let width = AccessWidth::decode(b).ok_or(DecodeError::InvalidAccessWidth {
                instruction: index,
                width: b,
            })?;
            Instruction::LoadScratch {
                dst: a,
                offset: operand,
                width,
            }
        }
        OP_STORE_SCRATCH => {
            if reserved_fields_are_nonzero_fn(true, true, false, true, false) {
                return Err(DecodeError::NonCanonicalEncoding { instruction: index });
            }
            let width = AccessWidth::decode(b).ok_or(DecodeError::InvalidAccessWidth {
                instruction: index,
                width: b,
            })?;
            Instruction::StoreScratch {
                offset: operand,
                src: a,
                width,
            }
        }
        OP_MOV_IMM => {
            if reserved_fields_are_nonzero_fn(true, false, false, false, true) {
                return Err(DecodeError::NonCanonicalEncoding { instruction: index });
            }
            Instruction::MovImm {
                dst: a,
                value: immediate,
            }
        }
        OP_ADD | OP_COMPARE_EQ => {
            if reserved_fields_are_nonzero_fn(true, true, true, false, false) {
                return Err(DecodeError::NonCanonicalEncoding { instruction: index });
            }
            if opcode == OP_ADD {
                Instruction::Add {
                    dst: a,
                    lhs: b,
                    rhs: c,
                }
            } else {
                Instruction::CompareEq {
                    dst: a,
                    lhs: b,
                    rhs: c,
                }
            }
        }
        OP_JUMP_IF_ZERO => {
            if reserved_fields_are_nonzero_fn(true, false, false, true, false) {
                return Err(DecodeError::NonCanonicalEncoding { instruction: index });
            }
            Instruction::JumpIfZero {
                condition: a,
                target: operand,
            }
        }
        OP_CALL_HELPER => {
            if reserved_fields_are_nonzero_fn(true, true, true, false, false) {
                return Err(DecodeError::NonCanonicalEncoding { instruction: index });
            }
            let helper = HelperId::decode(a).ok_or(DecodeError::InvalidHelper {
                instruction: index,
                helper: a,
            })?;
            Instruction::CallHelper {
                helper,
                arg0: b,
                arg1: c,
            }
        }
        OP_EXIT => {
            if reserved_fields_are_nonzero_fn(false, false, false, false, false) {
                return Err(DecodeError::NonCanonicalEncoding { instruction: index });
            }
            Instruction::Exit
        }
        _ => {
            return Err(DecodeError::InvalidOpcode {
                instruction: index,
                opcode,
            });
        }
    };
    Ok(instruction)
}

#[cfg(ktest)]
pub(super) fn encode(instruction: Instruction) -> [u8; INSTRUCTION_SIZE] {
    let mut raw = [0u8; INSTRUCTION_SIZE];
    let mut operand = 0u32;
    let mut immediate = 0u64;
    match instruction {
        Instruction::LoadContext { dst, field } => {
            raw[0] = OP_LOAD_CONTEXT;
            raw[1] = dst;
            raw[2] = field.encode();
        }
        Instruction::LoadScratch { dst, offset, width } => {
            raw[0] = OP_LOAD_SCRATCH;
            raw[1] = dst;
            raw[2] = width.encode();
            operand = offset;
        }
        Instruction::StoreScratch { offset, src, width } => {
            raw[0] = OP_STORE_SCRATCH;
            raw[1] = src;
            raw[2] = width.encode();
            operand = offset;
        }
        Instruction::MovImm { dst, value } => {
            raw[0] = OP_MOV_IMM;
            raw[1] = dst;
            immediate = value;
        }
        Instruction::Add { dst, lhs, rhs } => {
            raw[0] = OP_ADD;
            raw[1] = dst;
            raw[2] = lhs;
            raw[3] = rhs;
        }
        Instruction::CompareEq { dst, lhs, rhs } => {
            raw[0] = OP_COMPARE_EQ;
            raw[1] = dst;
            raw[2] = lhs;
            raw[3] = rhs;
        }
        Instruction::JumpIfZero { condition, target } => {
            raw[0] = OP_JUMP_IF_ZERO;
            raw[1] = condition;
            operand = target;
        }
        Instruction::CallHelper { helper, arg0, arg1 } => {
            raw[0] = OP_CALL_HELPER;
            raw[1] = helper.encode();
            raw[2] = arg0;
            raw[3] = arg1;
        }
        Instruction::Exit => raw[0] = OP_EXIT,
    }
    raw[4..8].copy_from_slice(&operand.to_le_bytes());
    raw[8..16].copy_from_slice(&immediate.to_le_bytes());
    raw
}
