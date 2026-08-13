// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;

use ostd::prelude::*;

use super::{
    interpreter::{self, SyscallContext},
    isa::{self, AccessWidth, ContextField, HelperId, Instruction},
    program::{Program, ProgramMetadata, VerifyError},
    syscall_observer::{
        self, SyscallHelpers, dropped_events, is_attached, pop_event, reset_events,
    },
};

const OPENAT: u64 = 257;
const PAYLOAD: u64 = 0x7473_6574_2d74_7865;

fn program_bytes(instructions: &[Instruction]) -> Vec<u8> {
    instructions
        .iter()
        .flat_map(|instruction| isa::encode(*instruction))
        .collect()
}

fn observer_program() -> Vec<u8> {
    program_bytes(&[
        Instruction::LoadContext {
            dst: 1,
            field: ContextField::SyscallNumber,
        },
        Instruction::MovImm {
            dst: 2,
            value: OPENAT,
        },
        Instruction::CompareEq {
            dst: 3,
            lhs: 1,
            rhs: 2,
        },
        Instruction::JumpIfZero {
            condition: 3,
            target: 9,
        },
        Instruction::MovImm {
            dst: 4,
            value: PAYLOAD,
        },
        Instruction::StoreScratch {
            offset: 0,
            src: 4,
            width: AccessWidth::Double,
        },
        Instruction::MovImm { dst: 5, value: 0 },
        Instruction::MovImm { dst: 6, value: 8 },
        Instruction::CallHelper {
            helper: HelperId::EmitEvent,
            arg0: 5,
            arg1: 6,
        },
        Instruction::Exit,
    ])
}

fn context(syscall_number: u64) -> SyscallContext {
    SyscallContext {
        syscall_number,
        process_id: 10,
        thread_id: 11,
        arguments: [1, 2, 3, 4, 5, 6],
        timestamp: 12,
    }
}

#[ktest]
fn observer_emits_only_for_matching_syscall() {
    reset_events();
    let bytes = observer_program();
    let program = Program::load(&bytes, ProgramMetadata::syscall_observer()).unwrap();

    interpreter::run(&program, &context(OPENAT + 1), &SyscallHelpers).unwrap();
    assert_eq!(pop_event(), None);

    interpreter::run(&program, &context(OPENAT), &SyscallHelpers).unwrap();
    let event = pop_event().unwrap();
    assert_eq!(event.syscall_number, OPENAT);
    assert_eq!(event.process_id, 10);
    assert_eq!(event.thread_id, 11);
    assert_eq!(event.timestamp, 12);
    assert_eq!(event.payload_len, 8);
    assert_eq!(&event.payload[..8], &PAYLOAD.to_le_bytes());
    assert_eq!(pop_event(), None);

    for _ in 0..65 {
        interpreter::run(&program, &context(OPENAT), &SyscallHelpers).unwrap();
    }
    let mut event_count = 0;
    while pop_event().is_some() {
        event_count += 1;
    }
    assert_eq!(event_count, 64);
    assert_eq!(dropped_events(), 1);
    reset_events();
}

#[ktest]
fn rejects_invalid_opcode() {
    let mut bytes = program_bytes(&[Instruction::Exit]);
    bytes[0] = 0xff;
    assert!(matches!(
        Program::load(&bytes, ProgramMetadata::syscall_observer()),
        Err(VerifyError::Decode(_))
    ));
}

#[ktest]
fn rejects_noncanonical_encoding() {
    let mut bytes = program_bytes(&[Instruction::Exit]);
    bytes[1] = 1;
    assert!(matches!(
        Program::load(&bytes, ProgramMetadata::syscall_observer()),
        Err(VerifyError::Decode(_))
    ));
}

#[ktest]
fn rejects_unknown_interface_version() {
    let bytes = program_bytes(&[Instruction::Exit]);
    let metadata = ProgramMetadata::new(1, 1, 2);
    assert!(matches!(
        Program::load(&bytes, metadata),
        Err(VerifyError::UnsupportedInterfaceVersion)
    ));
}

#[ktest]
fn rejects_invalid_register() {
    let bytes = program_bytes(&[
        Instruction::MovImm { dst: 8, value: 0 },
        Instruction::Exit,
    ]);
    assert!(matches!(
        Program::load(&bytes, ProgramMetadata::syscall_observer()),
        Err(VerifyError::InvalidRegister { register: 8, .. })
    ));
}

#[ktest]
fn rejects_backward_jump() {
    let bytes = program_bytes(&[
        Instruction::MovImm { dst: 1, value: 0 },
        Instruction::JumpIfZero {
            condition: 1,
            target: 0,
        },
        Instruction::Exit,
    ]);
    assert!(matches!(
        Program::load(&bytes, ProgramMetadata::syscall_observer()),
        Err(VerifyError::InvalidJump { .. })
    ));
}

#[ktest]
fn rejects_register_initialized_on_only_one_path() {
    let bytes = program_bytes(&[
        Instruction::LoadContext {
            dst: 1,
            field: ContextField::SyscallNumber,
        },
        Instruction::JumpIfZero {
            condition: 1,
            target: 3,
        },
        Instruction::MovImm { dst: 2, value: 1 },
        Instruction::Add {
            dst: 3,
            lhs: 1,
            rhs: 2,
        },
        Instruction::Exit,
    ]);
    assert!(matches!(
        Program::load(&bytes, ProgramMetadata::syscall_observer()),
        Err(VerifyError::UninitializedRegister { register: 2, .. })
    ));
}

#[ktest]
fn rejects_uninitialized_scratch_read() {
    let bytes = program_bytes(&[
        Instruction::LoadScratch {
            dst: 1,
            offset: 0,
            width: AccessWidth::Double,
        },
        Instruction::Exit,
    ]);
    assert!(matches!(
        Program::load(&bytes, ProgramMetadata::syscall_observer()),
        Err(VerifyError::UninitializedScratch { .. })
    ));
}

#[ktest]
fn rejects_scratch_out_of_bounds() {
    let bytes = program_bytes(&[
        Instruction::MovImm { dst: 1, value: 0 },
        Instruction::StoreScratch {
            offset: 255,
            src: 1,
            width: AccessWidth::Double,
        },
        Instruction::Exit,
    ]);
    assert!(matches!(
        Program::load(&bytes, ProgramMetadata::syscall_observer()),
        Err(VerifyError::ScratchOutOfBounds { .. })
    ));
}

#[ktest]
fn rejects_nonconstant_event_range() {
    let bytes = program_bytes(&[
        Instruction::LoadContext {
            dst: 1,
            field: ContextField::Argument(0),
        },
        Instruction::MovImm { dst: 2, value: 0 },
        Instruction::CallHelper {
            helper: HelperId::EmitEvent,
            arg0: 2,
            arg1: 1,
        },
        Instruction::Exit,
    ]);
    assert!(matches!(
        Program::load(&bytes, ProgramMetadata::syscall_observer()),
        Err(VerifyError::HelperArgumentMustBeConstant { .. })
    ));
}

#[ktest]
fn rejects_oversized_event() {
    let bytes = program_bytes(&[
        Instruction::MovImm { dst: 1, value: 0 },
        Instruction::StoreScratch {
            offset: 0,
            src: 1,
            width: AccessWidth::Double,
        },
        Instruction::MovImm { dst: 2, value: 0 },
        Instruction::MovImm { dst: 3, value: 65 },
        Instruction::CallHelper {
            helper: HelperId::EmitEvent,
            arg0: 2,
            arg1: 3,
        },
        Instruction::Exit,
    ]);
    assert!(matches!(
        Program::load(&bytes, ProgramMetadata::syscall_observer()),
        Err(VerifyError::InvalidHelperArguments { .. })
    ));
}

#[ktest]
fn rejects_path_without_exit() {
    let bytes = program_bytes(&[Instruction::MovImm { dst: 1, value: 0 }]);
    assert!(matches!(
        Program::load(&bytes, ProgramMetadata::syscall_observer()),
        Err(VerifyError::PathWithoutExit { .. })
    ));
}

#[ktest]
fn attach_and_detach_replace_program_atomically() {
    syscall_observer::detach();
    assert!(!is_attached());

    let bytes = observer_program();
    syscall_observer::load_and_attach(&bytes, ProgramMetadata::syscall_observer()).unwrap();
    assert!(is_attached());

    syscall_observer::detach();
    assert!(!is_attached());
}
