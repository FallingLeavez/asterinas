// SPDX-License-Identifier: MPL-2.0

//! Read-only system-call observer and its helper implementations.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use ostd::{
    sync::{RcuOption, SpinLock},
    timer::Jiffies,
};

use super::{
    interpreter::{self, HelperDispatcher, RuntimeError, SyscallContext},
    isa::{HelperId, SCRATCH_SIZE},
    program::{Program, ProgramMetadata, VerifyError},
};
use crate::context::Context;

const EVENT_PAYLOAD_SIZE: usize = 64;
const EVENT_QUEUE_CAPACITY: usize = 64;

static ATTACHED_PROGRAM: RcuOption<Arc<Program>> = RcuOption::new_none();
static ATTACH_LOCK: SpinLock<()> = SpinLock::new(());
static EVENTS: SpinLock<EventQueue> = SpinLock::new(EventQueue::new());
static DROPPED_EVENTS: AtomicU64 = AtomicU64::new(0);
static RUNTIME_ERRORS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ObserverEvent {
    pub syscall_number: u64,
    pub process_id: u64,
    pub thread_id: u64,
    pub timestamp: u64,
    pub payload_len: u8,
    pub payload: [u8; EVENT_PAYLOAD_SIZE],
}

impl ObserverEvent {
    const EMPTY: Self = Self {
        syscall_number: 0,
        process_id: 0,
        thread_id: 0,
        timestamp: 0,
        payload_len: 0,
        payload: [0; EVENT_PAYLOAD_SIZE],
    };
}

struct EventQueue {
    events: [ObserverEvent; EVENT_QUEUE_CAPACITY],
    head: usize,
    len: usize,
}

impl EventQueue {
    const fn new() -> Self {
        Self {
            events: [ObserverEvent::EMPTY; EVENT_QUEUE_CAPACITY],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, event: ObserverEvent) -> bool {
        if self.len == EVENT_QUEUE_CAPACITY {
            return false;
        }
        let tail = (self.head + self.len) % EVENT_QUEUE_CAPACITY;
        self.events[tail] = event;
        self.len += 1;
        true
    }

    #[cfg(ktest)]
    fn pop(&mut self) -> Option<ObserverEvent> {
        if self.len == 0 {
            return None;
        }
        let event = self.events[self.head];
        self.head = (self.head + 1) % EVENT_QUEUE_CAPACITY;
        self.len -= 1;
        Some(event)
    }

    #[cfg(ktest)]
    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

pub(super) struct SyscallHelpers;

impl HelperDispatcher for SyscallHelpers {
    fn call(
        &self,
        helper: HelperId,
        arg0: u64,
        arg1: u64,
        scratch: &[u8; SCRATCH_SIZE],
        context: &SyscallContext,
    ) -> Result<u64, RuntimeError> {
        match helper {
            HelperId::ReadTime => Ok(Jiffies::elapsed().as_u64()),
            HelperId::EmitEvent => {
                let range = interpreter::checked_range(arg0, arg1)?;
                if range.len() > EVENT_PAYLOAD_SIZE {
                    return Err(RuntimeError::ScratchOutOfBounds);
                }
                let mut event = ObserverEvent {
                    syscall_number: context.syscall_number,
                    process_id: context.process_id,
                    thread_id: context.thread_id,
                    timestamp: context.timestamp,
                    payload_len: range.len() as u8,
                    payload: [0; EVENT_PAYLOAD_SIZE],
                };
                event.payload[..range.len()].copy_from_slice(&scratch[range]);
                let Some(mut queue) = EVENTS.try_lock() else {
                    DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed);
                    return Ok(1);
                };
                if !queue.push(event) {
                    DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed);
                    return Ok(1);
                }
                Ok(0)
            }
        }
    }
}

/// Runs the attached observer once before the system-call handler.
pub(super) fn observe(syscall_number: u64, args: [u64; 6], ctx: &Context) {
    let guard = ATTACHED_PROGRAM.read();
    let Some(program) = guard.get() else {
        return;
    };
    let context = SyscallContext {
        syscall_number,
        process_id: u64::from(ctx.process.pid()),
        thread_id: u64::from(ctx.posix_thread.tid()),
        arguments: args,
        timestamp: Jiffies::elapsed().as_u64(),
    };
    if interpreter::run(program, &context, &SyscallHelpers).is_err() {
        RUNTIME_ERRORS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Loads and atomically attaches a system-call observer.
#[cfg_attr(
    not(ktest),
    expect(dead_code, reason = "reserved for the future user-facing loader")
)]
pub(super) fn load_and_attach(
    bytes: &[u8],
    metadata: ProgramMetadata,
) -> Result<(), VerifyError> {
    let program = Arc::new(Program::load(bytes, metadata)?);
    let _guard = ATTACH_LOCK.lock();
    ATTACHED_PROGRAM.update(Some(program));
    Ok(())
}

/// Atomically detaches the current observer.
#[cfg_attr(
    not(ktest),
    expect(dead_code, reason = "reserved for the future user-facing loader")
)]
pub(super) fn detach() {
    let _guard = ATTACH_LOCK.lock();
    ATTACHED_PROGRAM.update(None);
}

#[cfg(ktest)]
pub(super) fn pop_event() -> Option<ObserverEvent> {
    EVENTS.lock().pop()
}

#[cfg(ktest)]
pub(super) fn reset_events() {
    EVENTS.lock().clear();
    DROPPED_EVENTS.store(0, Ordering::Relaxed);
    RUNTIME_ERRORS.store(0, Ordering::Relaxed);
}

#[cfg(ktest)]
pub(super) fn dropped_events() -> u64 {
    DROPPED_EVENTS.load(Ordering::Relaxed)
}

#[cfg(ktest)]
pub(super) fn is_attached() -> bool {
    ATTACHED_PROGRAM.read().get().is_some()
}
