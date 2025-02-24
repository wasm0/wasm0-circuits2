//! Definition of each opcode of the EVM.
use core::fmt::Debug;

use ethers_core::utils::get_contract_address;

use address::Address;
use balance::Balance;
use calldatacopy::Calldatacopy;
use calldataload::Calldataload;
use calldatasize::Calldatasize;
use caller::Caller;
use callop::CallOpcode;
use callvalue::Callvalue;
use codecopy::Codecopy;
use codesize::Codesize;
use error_invalid_jump::InvalidJump;
use error_oog_call::OOGCall;
use error_oog_log::ErrorOOGLog;
use error_oog_sload_sstore::OOGSloadSstore;
use error_return_data_outofbound::ErrorReturnDataOutOfBound;
use error_write_protection::ErrorWriteProtection;
use eth_types::{evm_types::{GasCost, MAX_REFUND_QUOTIENT_OF_GAS_USED}, evm_unimplemented, GethExecStep, GethExecTrace, StackWord, ToAddress, ToWord, Word};
use eth_types::evm_types::MemoryAddress;
use extcodecopy::Extcodecopy;
use extcodesize::Extcodesize;
use gasprice::GasPrice;
use number::Number;
use origin::Origin;
use return_revert::ReturnRevert;
use returndatacopy::Returndatacopy;
use returndatasize::Returndatasize;
use selfbalance::Selfbalance;
use stackonlyop::StackOnlyOpcode;
use stacktomemoryop::{StackToMemoryOpcode, STACK_TO_MEMORY_TYPE_U256, STACK_TO_MEMORY_TYPE_U64};
use stop::Stop;
use wasm_break::WasmBreakOpcode;
use wasm_call::WasmCallOpcode;
use wasm_global::WasmGlobalOpcode;
use wasm_local::WasmLocalOpcode;

use crate::{
    circuit_input_builder::{CircuitInputStateRef, ExecStep},
    error::{ExecError, OogError},
    Error,
    evm::OpcodeId,
    operation::{
        AccountField, AccountOp, CallContextField, RW, TxAccessListAccountOp, TxReceiptField,
        TxRefundOp,
    },
};
use crate::error::{ContractAddressCollisionError, DepthError, InsufficientBalanceError, NonceUintOverflowError};
use crate::evm::opcodes::create::Create;
use crate::evm::opcodes::extcodehash::Extcodehash;
use crate::precompile::is_precompiled;
use crate::state_db::CodeDB;
use crate::util::CHECK_MEM_STRICT;
use crate::wasm::opcodes::error_codestore::ErrorCodeStore;
use crate::wasm::opcodes::error_invalid_creation_code::ErrorCreationCode;
use crate::wasm::opcodes::error_oog_account_access::ErrorOOGAccountAccess;
use crate::wasm::opcodes::error_oog_dynamic_memory::OOGDynamicMemory;
use crate::wasm::opcodes::error_oog_memory_copy::OOGMemoryCopy;
use crate::wasm::opcodes::error_precompile_failed::PrecompileFailed;
use crate::wasm::opcodes::logs::Log;
use crate::wasm::opcodes::sha3::Sha3;
use crate::wasm::opcodes::sload::Sload;
use crate::wasm::opcodes::sstore::Sstore;

#[cfg(any(feature = "test", test))]
pub use self::sha3::sha3_tests::{gen_sha3_code, MemoryKind};

mod address;
mod balance;
mod calldatacopy;
mod calldataload;
mod calldatasize;
mod caller;
mod callop;
mod callvalue;
mod codecopy;
mod codesize;
mod create;
mod extcodecopy;
mod extcodehash;
mod extcodesize;
mod gasprice;
mod logs;
mod number;
mod origin;
mod precompiles;
mod return_revert;
mod returndatacopy;
mod returndatasize;
mod selfbalance;
mod sha3;
mod sload;
mod sstore;
mod stackonlyop;
mod stacktomemoryop;
mod stop;

mod error_codestore;
mod error_contract_address_collision;
mod error_invalid_creation_code;
mod error_invalid_jump;
mod error_oog_account_access;
mod error_oog_call;
mod error_oog_dynamic_memory;
mod error_oog_log;
mod error_oog_memory_copy;
mod error_oog_sload_sstore;
mod error_precompile_failed;
mod error_return_data_outofbound;
mod error_write_protection;

#[cfg(test)]
mod memory_expansion_test;
#[cfg(feature = "test")]
pub use callop::tests::PrecompileCallArgs;

mod wasm_call;
mod wasm_global;
mod wasm_local;
mod wasm_break;

/// Generic opcode trait which defines the logic of the
/// [`Operation`](crate::operation::Operation) that should be generated for one
/// or multiple [`ExecStep`](crate::circuit_input_builder::ExecStep) depending
/// of the [`OpcodeId`] it contains.
pub trait Opcode: Debug {

    /// Generate the associated [`MemoryOp`](crate::operation::MemoryOp)s,
    /// [`StackOp`](crate::operation::StackOp)s, and
    /// [`StorageOp`](crate::operation::StorageOp)s associated to the Opcode
    /// is implemented for.
    fn gen_associated_ops(
        _state: &mut CircuitInputStateRef,
        _geth_steps: &[GethExecStep],
    ) -> Result<Vec<ExecStep>, Error>;
}

#[derive(Debug, Copy, Clone)]
struct Dummy;

impl Opcode for Dummy {
    fn gen_associated_ops(
        state: &mut CircuitInputStateRef,
        geth_steps: &[GethExecStep],
    ) -> Result<Vec<ExecStep>, Error> {
        Ok(vec![state.new_step(&geth_steps[0])?])
    }
}

type FnGenAssociatedOps = fn(
    state: &mut CircuitInputStateRef,
    geth_steps: &[GethExecStep],
) -> Result<Vec<ExecStep>, Error>;

fn fn_gen_associated_ops(opcode_id: &OpcodeId) -> FnGenAssociatedOps {
    match opcode_id {
        // WASM opcodes
        OpcodeId::Unreachable => Stop::gen_associated_ops,
        // OpcodeId::Nop => Dummy::gen_associated_ops,
        // OpcodeId::Block => Dummy::gen_associated_ops,
        // OpcodeId::Loop => Dummy::gen_associated_ops,
        // OpcodeId::If => Dummy::gen_associated_ops,
        // OpcodeId::Else => Dummy::gen_associated_ops,
        OpcodeId::End => Stop::gen_associated_ops,
        // OpcodeId::Br => Dummy::gen_associated_ops,
        // OpcodeId::BrIf => Dummy::gen_associated_ops,
        // OpcodeId::BrTable => Dummy::gen_associated_ops,
        // OpcodeId::Return => Dummy::gen_associated_ops,
        // OpcodeId::Call => Dummy::gen_associated_ops,
        // OpcodeId::CallIndirect => Dummy::gen_associated_ops,
        // OpcodeId::Drop => Dummy::gen_associated_ops,
        // OpcodeId::Select => Dummy::gen_associated_ops,
        // OpcodeId::GetLocal => Dummy::gen_associated_ops,
        // OpcodeId::SetLocal => Dummy::gen_associated_ops,
        // OpcodeId::TeeLocal => Dummy::gen_associated_ops,
        // OpcodeId::GetGlobal => Dummy::gen_associated_ops,
        // OpcodeId::SetGlobal => Dummy::gen_associated_ops,
        // OpcodeId::I32Load => Dummy::gen_associated_ops,
        // OpcodeId::I64Load => Dummy::gen_associated_ops,
        // OpcodeId::F32Load => Dummy::gen_associated_ops,
        // OpcodeId::F64Load => Dummy::gen_associated_ops,
        // OpcodeId::I32Load8S => Dummy::gen_associated_ops,
        // OpcodeId::I32Load8U => Dummy::gen_associated_ops,
        // OpcodeId::I32Load16S => Dummy::gen_associated_ops,
        // OpcodeId::I32Load16U => Dummy::gen_associated_ops,
        // OpcodeId::I64Load8S => Dummy::gen_associated_ops,
        // OpcodeId::I64Load8U => Dummy::gen_associated_ops,
        // OpcodeId::I64Load16S => Dummy::gen_associated_ops,
        // OpcodeId::I64Load16U => Dummy::gen_associated_ops,
        // OpcodeId::I64Load32S => Dummy::gen_associated_ops,
        // OpcodeId::I64Load32U => Dummy::gen_associated_ops,
        // OpcodeId::I32Store => Dummy::gen_associated_ops,
        // OpcodeId::I64Store => Dummy::gen_associated_ops,
        // OpcodeId::F32Store => Dummy::gen_associated_ops,
        // OpcodeId::F64Store => Dummy::gen_associated_ops,
        // OpcodeId::I32Store8 => Dummy::gen_associated_ops,
        // OpcodeId::I32Store16 => Dummy::gen_associated_ops,
        // OpcodeId::I64Store8 => Dummy::gen_associated_ops,
        // OpcodeId::I64Store16 => Dummy::gen_associated_ops,
        // OpcodeId::I64Store32 => Dummy::gen_associated_ops,
        // OpcodeId::CurrentMemory => Dummy::gen_associated_ops,
        // OpcodeId::GrowMemory => Dummy::gen_associated_ops,
        OpcodeId::I32Const |
        OpcodeId::I64Const => StackOnlyOpcode::<0, 1>::gen_associated_ops,
        // WASM binary opcodes

        OpcodeId::I32Eq |
        OpcodeId::I32Ne |

        OpcodeId::I32LtS |
        OpcodeId::I32GtS |
        OpcodeId::I32LeS |
        OpcodeId::I32GeS |

        OpcodeId::I32LtU |
        OpcodeId::I32GtU |
        OpcodeId::I32LeU |
        OpcodeId::I32GeU |

        OpcodeId::I64Eq |
        OpcodeId::I64Ne |

        OpcodeId::I64LtS |
        OpcodeId::I64GtS |
        OpcodeId::I64LeS |
        OpcodeId::I64GeS |

        OpcodeId::I64LtU |
        OpcodeId::I64GtU |
        OpcodeId::I64LeU |
        OpcodeId::I64GeU |

        OpcodeId::I32Add |
        OpcodeId::I32Sub |
        OpcodeId::I32Mul |
        OpcodeId::I32DivS |
        OpcodeId::I32DivU |
        OpcodeId::I32RemS |
        OpcodeId::I32RemU |
        OpcodeId::I32And |
        OpcodeId::I32Or |
        OpcodeId::I32Xor |
        OpcodeId::I32Shl |
        OpcodeId::I32ShrS |
        OpcodeId::I32ShrU |
        OpcodeId::I32Rotl |
        OpcodeId::I32Rotr |
        OpcodeId::I64Add |
        OpcodeId::I64Sub |
        OpcodeId::I64Mul |
        OpcodeId::I64DivS |
        OpcodeId::I64DivU |
        OpcodeId::I64RemS |
        OpcodeId::I64RemU |
        OpcodeId::I64And |
        OpcodeId::I64Or |
        OpcodeId::I64Xor |
        OpcodeId::I64Shl |
        OpcodeId::I64ShrS |
        OpcodeId::I64ShrU |
        OpcodeId::I64Rotl |
        OpcodeId::I64Rotr => StackOnlyOpcode::<2, 1>::gen_associated_ops,

        // WASM load store like opcodes (like unary).
        OpcodeId::I32Load |
        OpcodeId::I32Load8S |
        OpcodeId::I32Load8U |
        OpcodeId::I32Load16S |
        OpcodeId::I32Load16U |
        OpcodeId::I64Load |
        OpcodeId::I64Load8S |
        OpcodeId::I64Load8U |
        OpcodeId::I64Load16S |
        OpcodeId::I64Load16U |
        OpcodeId::I64Load32S |
        OpcodeId::I64Load32U => StackOnlyOpcode::<1, 1>::gen_associated_ops,

        // WASM unary opcodes
        OpcodeId::I64ExtendUI32 |
        OpcodeId::I64ExtendSI32 |
        OpcodeId::I32WrapI64 |
        OpcodeId::I32Ctz |
        OpcodeId::I64Ctz |
        OpcodeId::I32Clz |
        OpcodeId::I64Clz |
        OpcodeId::I32Popcnt |
        OpcodeId::I64Popcnt => StackOnlyOpcode::<1, 1>::gen_associated_ops,

        // WASM global opcodes
        OpcodeId::SetGlobal |
        OpcodeId::GetGlobal => WasmGlobalOpcode::gen_associated_ops,
        // WASM local opcodes
        OpcodeId::SetLocal |
        OpcodeId::GetLocal |
        OpcodeId::TeeLocal => WasmLocalOpcode::gen_associated_ops,
        // call opcodes
        OpcodeId::Call |
        OpcodeId::CallIndirect => WasmCallOpcode::gen_associated_ops,
        // control flow opcodes (PC)
        OpcodeId::Return |
        OpcodeId::Br |
        OpcodeId::BrIf |
        OpcodeId::BrTable => WasmBreakOpcode::gen_associated_ops,

        // WASM select like opcodes.
        OpcodeId::Select => StackOnlyOpcode::<3, 1>::gen_associated_ops,

        // WASM store like ops.
        OpcodeId::I32Store |
        OpcodeId::I32Store8 |
        OpcodeId::I32Store16 |
        OpcodeId::I64Store |
        OpcodeId::I64Store8 |
        OpcodeId::I64Store16 |
        OpcodeId::I64Store32 => StackOnlyOpcode::<2, 0>::gen_associated_ops,

        // WASM test opcodes
        OpcodeId::I32Eqz | OpcodeId::I64Eqz => StackOnlyOpcode::<1, 1>::gen_associated_ops,

        OpcodeId::Drop => StackOnlyOpcode::<1, 0>::gen_associated_ops,

        // EVM opcodes
        OpcodeId::STOP => Stop::gen_associated_ops,
        OpcodeId::SHA3 => Sha3::gen_associated_ops,
        OpcodeId::ADDRESS => Address::gen_associated_ops,
        OpcodeId::BALANCE => Balance::gen_associated_ops,
        OpcodeId::ORIGIN => Origin::gen_associated_ops,
        OpcodeId::CALLER => Caller::gen_associated_ops,
        OpcodeId::CALLVALUE => Callvalue::gen_associated_ops,
        OpcodeId::CALLDATASIZE => Calldatasize::gen_associated_ops,
        OpcodeId::CALLDATALOAD => Calldataload::gen_associated_ops,
        OpcodeId::CALLDATACOPY => Calldatacopy::gen_associated_ops,
        OpcodeId::GASPRICE => GasPrice::gen_associated_ops,
        OpcodeId::CODECOPY => Codecopy::gen_associated_ops,
        OpcodeId::CODESIZE => Codesize::gen_associated_ops,
        OpcodeId::EXTCODESIZE => Extcodesize::gen_associated_ops,
        OpcodeId::EXTCODECOPY => Extcodecopy::gen_associated_ops,
        OpcodeId::RETURNDATASIZE => Returndatasize::gen_associated_ops,
        OpcodeId::RETURNDATACOPY => Returndatacopy::gen_associated_ops,
        OpcodeId::EXTCODEHASH => Extcodehash::gen_associated_ops,
        OpcodeId::BLOCKHASH => StackToMemoryOpcode::<1, STACK_TO_MEMORY_TYPE_U256>::gen_associated_ops,
        OpcodeId::COINBASE => StackToMemoryOpcode::<0>::gen_associated_ops,
        OpcodeId::TIMESTAMP => StackToMemoryOpcode::<0>::gen_associated_ops,
        OpcodeId::NUMBER => Number::gen_associated_ops,
        OpcodeId::DIFFICULTY => StackToMemoryOpcode::<0>::gen_associated_ops,
        OpcodeId::GASLIMIT => StackToMemoryOpcode::<0>::gen_associated_ops,
        OpcodeId::CHAINID => StackToMemoryOpcode::<0>::gen_associated_ops,
        OpcodeId::SELFBALANCE => Selfbalance::gen_associated_ops,
        OpcodeId::BASEFEE => StackToMemoryOpcode::<0>::gen_associated_ops,
        OpcodeId::SLOAD => Sload::gen_associated_ops,
        OpcodeId::SSTORE => Sstore::gen_associated_ops,
        OpcodeId::PC => StackToMemoryOpcode::<0, STACK_TO_MEMORY_TYPE_U64>::gen_associated_ops,
        OpcodeId::MSIZE => StackToMemoryOpcode::<0, STACK_TO_MEMORY_TYPE_U64>::gen_associated_ops,
        OpcodeId::GAS => StackToMemoryOpcode::<0, STACK_TO_MEMORY_TYPE_U64>::gen_associated_ops,
        OpcodeId::JUMPDEST => Dummy::gen_associated_ops,
        OpcodeId::LOG0 => Log::<0>::gen_associated_ops,
        OpcodeId::LOG1 => Log::<1>::gen_associated_ops,
        OpcodeId::LOG2 => Log::<2>::gen_associated_ops,
        OpcodeId::LOG3 => Log::<3>::gen_associated_ops,
        OpcodeId::LOG4 => Log::<4>::gen_associated_ops,
        OpcodeId::CALL | OpcodeId::CALLCODE => CallOpcode::<true>::gen_associated_ops,
        OpcodeId::DELEGATECALL | OpcodeId::STATICCALL => CallOpcode::<false>::gen_associated_ops,
        OpcodeId::RETURN | OpcodeId::REVERT => ReturnRevert::gen_associated_ops,
        OpcodeId::SELFDESTRUCT => {
            evm_unimplemented!("Using dummy gen_selfdestruct_ops for opcode SELFDESTRUCT");
            DummySelfDestruct::gen_associated_ops
        }
        // OpcodeId::CREATE => {
        //     evm_unimplemented!("Using dummy gen_create_ops for opcode {:?}", opcode_id);
        //     DummyCreate::<false>::gen_associated_ops
        // }
        // OpcodeId::CREATE2 => {
        //     evm_unimplemented!("Using dummy gen_create_ops for opcode {:?}", opcode_id);
        //     DummyCreate::<true>::gen_associated_ops
        // }
        _ => {
            evm_unimplemented!("Using dummy gen_associated_ops for opcode {:?}", opcode_id);
            Dummy::gen_associated_ops
        }
    }
}

fn fn_gen_error_state_associated_ops(
    geth_step: &GethExecStep,
    error: &ExecError,
) -> Option<FnGenAssociatedOps> {
    match error {
        ExecError::InvalidJump => Some(InvalidJump::gen_associated_ops),
        ExecError::InvalidOpcode => Some(StackOnlyOpcode::<0, 0>::gen_associated_ops),
        // Depth error could occur in CALL, CALLCODE, DELEGATECALL and STATICCALL.
        ExecError::Depth(DepthError::Call) => match geth_step.op {
            OpcodeId::CALL | OpcodeId::CALLCODE => Some(CallOpcode::<true>::gen_associated_ops),
            OpcodeId::DELEGATECALL | OpcodeId::STATICCALL => {
                Some(CallOpcode::<false>::gen_associated_ops)
            }
            op => unreachable!("ErrDepth cannot occur in {op}"),
        },
        // Depth error could occur in CREATE and CREATE2.
        ExecError::Depth(DepthError::Create) => Some(Create::<false>::gen_associated_ops),
        ExecError::Depth(DepthError::Create2) => Some(Create::<true>::gen_associated_ops),
        ExecError::OutOfGas(OogError::Call) => Some(OOGCall::gen_associated_ops),
        ExecError::OutOfGas(OogError::Constant) => {
            Some(StackOnlyOpcode::<0, 0, true>::gen_associated_ops)
        }
        ExecError::OutOfGas(OogError::Create) => {
            Some(StackOnlyOpcode::<4, 0, true>::gen_associated_ops)
        }
        ExecError::OutOfGas(OogError::Log) => Some(ErrorOOGLog::gen_associated_ops),
        ExecError::OutOfGas(OogError::DynamicMemoryExpansion) => {
            Some(OOGDynamicMemory::gen_associated_ops)
        }
        ExecError::OutOfGas(OogError::StaticMemoryExpansion) => {
            Some(StackOnlyOpcode::<1, 0, true>::gen_associated_ops)
        }
        ExecError::OutOfGas(OogError::Exp) => {
            Some(StackOnlyOpcode::<2, 0, true>::gen_associated_ops)
        }
        ExecError::OutOfGas(OogError::MemoryCopy) => Some(OOGMemoryCopy::gen_associated_ops),
        ExecError::OutOfGas(OogError::Sha3) => {
            Some(StackOnlyOpcode::<2, 0, true>::gen_associated_ops)
        }
        ExecError::OutOfGas(OogError::SloadSstore) => Some(OOGSloadSstore::gen_associated_ops),
        ExecError::OutOfGas(OogError::AccountAccess) => {
            Some(ErrorOOGAccountAccess::gen_associated_ops)
        }
        // ExecError::
        ExecError::StackOverflow => Some(StackOnlyOpcode::<0, 0, true>::gen_associated_ops),
        ExecError::StackUnderflow => Some(StackOnlyOpcode::<0, 0, true>::gen_associated_ops),
        ExecError::CodeStoreOutOfGas => Some(ErrorCodeStore::gen_associated_ops),
        ExecError::MaxCodeSizeExceeded => Some(ErrorCodeStore::gen_associated_ops),
        // call & callcode can encounter InsufficientBalance error, Use pop-7 generic CallOpcode
        ExecError::InsufficientBalance(InsufficientBalanceError::Call) => {
            Some(CallOpcode::<true>::gen_associated_ops)
        }
        // create & create2 can encounter insufficient balance.
        ExecError::InsufficientBalance(InsufficientBalanceError::Create) => {
            Some(Create::<false>::gen_associated_ops)
        }
        ExecError::InsufficientBalance(InsufficientBalanceError::Create2) => {
            Some(Create::<true>::gen_associated_ops)
        }
        ExecError::PrecompileFailed => Some(PrecompileFailed::gen_associated_ops),
        ExecError::WriteProtection => Some(ErrorWriteProtection::gen_associated_ops),
        ExecError::ReturnDataOutOfBounds => Some(ErrorReturnDataOutOfBound::gen_associated_ops),
        // create & create2 can encounter contract address collision.
        ExecError::ContractAddressCollision(ContractAddressCollisionError::Create) => {
            Some(Create::<false>::gen_associated_ops)
        }
        ExecError::ContractAddressCollision(ContractAddressCollisionError::Create2) => {
            Some(Create::<true>::gen_associated_ops)
        }
        // create & create2 can encounter nonce uint overflow.
        ExecError::NonceUintOverflow(NonceUintOverflowError::Create) => {
            Some(Create::<false>::gen_associated_ops)
        }
        ExecError::NonceUintOverflow(NonceUintOverflowError::Create2) => {
            Some(Create::<true>::gen_associated_ops)
        }
        ExecError::InvalidCreationCode => Some(ErrorCreationCode::gen_associated_ops),
        // more future errors place here
        _ => {
            evm_unimplemented!("TODO: error state {:?} not implemented", error);
            None
        }
    }
}

#[allow(clippy::collapsible_else_if)]
/// Generate the associated operations according to the particular
/// [`OpcodeId`].
pub fn gen_associated_ops(
    opcode_id: &OpcodeId,
    state: &mut CircuitInputStateRef,
    geth_steps: &[GethExecStep],
) -> Result<Vec<ExecStep>, Error> {
    let memory_enabled = !geth_steps.iter().all(|s| s.memory.is_empty());
    if memory_enabled {
        let check_level = if *CHECK_MEM_STRICT { 2 } else { 0 }; // 0: no check, 1: check and log error and fix, 2: check and assert_eq
        if check_level >= 1 {
            #[allow(clippy::collapsible_else_if)]
            if state.call_ctx()?.memory != geth_steps[0].global_memory {
                log::error!(
                    "wrong mem before {:?}. len in state {}, len in step {}",
                    opcode_id,
                    &state.call_ctx()?.memory.len(),
                    &geth_steps[0].memory.len(),
                );
                log::error!("state mem {:?}", &state.call_ctx()?.memory);
                log::error!("step  mem {:?}", &geth_steps[0].memory);

                for i in 0..std::cmp::min(
                    state.call_ctx()?.memory.0.len(),
                    geth_steps[0].global_memory.0.len(),
                ) {
                    let state_mem = state.call_ctx()?.memory.0[i];
                    let step_mem = geth_steps[0].global_memory.0[i];
                    if state_mem != step_mem {
                        log::error!(
                            "diff at {}: state {:?} != step {:?}",
                            i,
                            state_mem,
                            step_mem
                        );
                    }
                }
                if check_level >= 2 {
                    panic!("mem wrong");
                }
                state.call_ctx_mut()?.memory = geth_steps[0].global_memory.clone();
            }
        }
    }

    // check if have error
    let geth_step = &geth_steps[0];
    let mut exec_step = state.new_step(geth_step)?;
    let next_step = if geth_steps.len() > 1 {
        Some(&geth_steps[1])
    } else {
        None
    };
    if let Some(exec_error) = state.get_step_err(geth_step, next_step).unwrap() {
        log::warn!(
            "geth error {:?} occurred in  {:?} at pc {:?}",
            exec_error,
            geth_step.op,
            geth_step.pc,
        );

        exec_step.error = Some(exec_error.clone());
        // TODO: after more error state handled, refactor all error handling in
        // fn_gen_error_state_associated_ops method
        // For exceptions that have been implemented
        if let Some(fn_gen_error_ops) = fn_gen_error_state_associated_ops(geth_step, &exec_error) {
            let mut steps = fn_gen_error_ops(state, geth_steps)?;
            if let Some(e) = &steps[0].error {
                debug_assert_eq!(&exec_error, e);
            }
            steps[0].error = Some(exec_error.clone());
            return Ok(steps);
        } else {
            // For exceptions that fail to enter next call context, we need
            // to restore call context of current caller
            let mut need_restore = true;

            // For exceptions that already enter next call context, but fail immediately
            // (e.g. Depth, InsufficientBalance), we still need to parse the call.
            if geth_step.op.is_call_or_create()
                && !matches!(exec_error, ExecError::OutOfGas(OogError::Create))
            {
                let call = state.parse_call(geth_step)?;
                state.push_call(call);
                need_restore = false;
            }

            state.handle_return(&mut exec_step, geth_steps, need_restore)?;
            return Ok(vec![exec_step]);
        }
    }
    // if no errors, continue as normal
    let fn_gen_associated_ops = fn_gen_associated_ops(opcode_id);
    let res = fn_gen_associated_ops(state, geth_steps)?;
    // copy global memory dump into call context
    if state.has_call() {
        let call_ctx = state.call_ctx_mut()?;
        if geth_steps.len() > 1 {
            call_ctx.memory = geth_steps[1].global_memory.clone();
        } else if geth_steps.len() > 0 {
            call_ctx.memory = geth_steps[0].global_memory.clone();
        }
    }
    Ok(res)
}

pub fn gen_begin_tx_ops(
    state: &mut CircuitInputStateRef,
    geth_trace: &GethExecTrace,
) -> Result<(), Error> {
    let mut exec_step = state.new_begin_tx_step();
    let call = state.call()?.clone();

    for (field, value) in [
        (CallContextField::TxId, state.tx_ctx.id().into()),
        (
            CallContextField::RwCounterEndOfReversion,
            call.rw_counter_end_of_reversion.into(),
        ),
        (
            CallContextField::IsPersistent,
            (call.is_persistent as usize).into(),
        ),
        (CallContextField::IsSuccess, call.is_success.to_word()),
    ] {
        state.call_context_write(&mut exec_step, call.call_id, field, value);
    }

    // Increase caller's nonce
    let caller_address = call.caller_address;
    let mut nonce_prev = state.sdb.get_account(&caller_address).1.nonce;
    debug_assert!(nonce_prev <= state.tx.nonce.into());
    while nonce_prev < state.tx.nonce.into() {
        nonce_prev = state.sdb.increase_nonce(&caller_address).into();
        log::warn!("[debug] increase nonce to {}", nonce_prev);
    }
    state.account_write(
        &mut exec_step,
        caller_address,
        AccountField::Nonce,
        nonce_prev + 1,
        nonce_prev,
    )?;

    // Add caller, callee and coinbase (only for Shanghai) to access list.
    #[cfg(feature = "shanghai")]
    let accessed_addresses = [
        call.caller_address,
        call.address,
        state
            .block
            .headers
            .get(&state.tx.block_num)
            .unwrap()
            .coinbase,
    ];
    #[cfg(not(feature = "shanghai"))]
    let accessed_addresses = [call.caller_address, call.address];
    for address in accessed_addresses {
        let is_warm_prev = !state.sdb.add_account_to_access_list(address);
        state.tx_accesslist_account_write(
            &mut exec_step,
            state.tx_ctx.id(),
            address,
            true,
            is_warm_prev,
        )?;
    }

    // Calculate intrinsic gas cost
    let call_data_gas_cost = state
        .tx
        .input
        .iter()
        .fold(0, |acc, byte| acc + if *byte == 0 { 4 } else { 16 });
    let intrinsic_gas_cost = if state.tx.is_create() {
        GasCost::CREATION_TX.as_u64()
    } else {
        GasCost::TX.as_u64()
    } + call_data_gas_cost;
    exec_step.gas_cost = GasCost(intrinsic_gas_cost);

    // Get code_hash of callee
    // FIXME: call with value to precompile will cause the codehash of precompile
    // address to `CodeDB::empty_code_hash()`. FIXME: we should have a
    // consistent codehash for precompile contract.
    let callee_account = &state.sdb.get_account(&call.address).1.clone();
    let is_precompile = is_precompiled(&call.address);
    let callee_exists = !callee_account.is_empty() || is_precompile;
    if !callee_exists && call.value.is_zero() {
        state.sdb.get_account_mut(&call.address).1.storage.clear();
    }
    if state.tx.is_create()
        && ((!callee_account.code_hash.is_zero()
            && !callee_account.code_hash.eq(&CodeDB::empty_code_hash()))
            || !callee_account.nonce.is_zero())
    {
        unimplemented!("deployment collision");
    }
    let (callee_code_hash, is_empty_code_hash) = match (state.tx.is_create(), callee_exists) {
        (true, _) => (call.code_hash.to_word(), false),
        (_, true) => {
            debug_assert_eq!(
                callee_account.code_hash, call.code_hash,
                "callee account's code hash: {:?}, call's code hash: {:?}",
                callee_account.code_hash, call.code_hash
            );
            (
                call.code_hash.to_word(),
                call.code_hash == CodeDB::empty_code_hash(),
            )
        }
        (_, false) => (Word::zero(), true),
    };
    if !is_precompile && !call.is_create() {
        state.account_read(
            &mut exec_step,
            call.address,
            AccountField::CodeHash,
            callee_code_hash,
        );
    }

    // Transfer with fee
    let fee = state.tx.gas_price * state.tx.gas + state.tx_ctx.l1_fee;
    state.transfer_with_fee(
        &mut exec_step,
        call.caller_address,
        call.address,
        callee_exists,
        call.is_create(),
        call.value,
        Some(fee),
    )?;

    // In case of contract creation we wish to verify the correctness of the
    // contract's address (callee). This address is defined as:
    //
    // Keccak256(RLP([tx_caller, tx_nonce]))[12:]
    //
    // We feed the RLP-encoded bytes to the block's SHA3 inputs, which gets assigned
    // to the Keccak circuit, so that the BeginTxGadget can do a lookup to the
    // Keccak table and verify the contract address.
    if state.tx.is_create() {
        state.block.sha3_inputs.push({
            let mut stream = ethers_core::utils::rlp::RlpStream::new();
            stream.begin_list(2);
            stream.append(&caller_address);
            stream.append(&nonce_prev);
            stream.out().to_vec()
        });
    }

    // There are 4 branches from here.
    match (call.is_create(), is_precompile, is_empty_code_hash) {
        // 1. Creation transaction.
        (true, _, _) => {
            state.push_op_reversible(
                &mut exec_step,
                AccountOp {
                    address: call.address,
                    field: AccountField::Nonce,
                    value: 1.into(),
                    value_prev: 0.into(),
                },
            )?;
            for (field, value) in [
                (CallContextField::Depth, call.depth.into()),
                (
                    CallContextField::CallerAddress,
                    call.caller_address.to_word(),
                ),
                (
                    CallContextField::CalleeAddress,
                    get_contract_address(caller_address, nonce_prev).to_word(),
                ),
                (
                    CallContextField::CallDataOffset,
                    call.call_data_offset.into(),
                ),
                (
                    CallContextField::CallDataLength,
                    state.tx.input.len().into(),
                ),
                (CallContextField::Value, call.value),
                (CallContextField::IsStatic, (call.is_static as usize).into()),
                (CallContextField::LastCalleeId, 0.into()),
                (CallContextField::LastCalleeReturnDataOffset, 0.into()),
                (CallContextField::LastCalleeReturnDataLength, 0.into()),
                (CallContextField::IsRoot, 1.into()),
                (CallContextField::IsCreate, 1.into()),
                (CallContextField::CodeHash, call.code_hash.to_word()),
            ] {
                state.call_context_write(&mut exec_step, call.call_id, field, value);
            }
        }
        // 2. Call to precompiled.
        (_, true, _) => (),
        (_, _, is_empty_code_hash) => {
            // 3. Call to account with empty code (is_empty_code_hash == true).
            // 4. Call to account with non-empty code (is_empty_code_hash == false).
            if !is_empty_code_hash {
                for (field, value) in [
                    (CallContextField::Depth, call.depth.into()),
                    (
                        CallContextField::CallerAddress,
                        call.caller_address.to_word(),
                    ),
                    (CallContextField::CalleeAddress, call.address.to_word()),
                    (
                        CallContextField::CallDataOffset,
                        call.call_data_offset.into(),
                    ),
                    (
                        CallContextField::CallDataLength,
                        call.call_data_length.into(),
                    ),
                    (CallContextField::Value, call.value),
                    (CallContextField::IsStatic, (call.is_static as usize).into()),
                    (CallContextField::LastCalleeId, 0.into()),
                    (CallContextField::LastCalleeReturnDataOffset, 0.into()),
                    (CallContextField::LastCalleeReturnDataLength, 0.into()),
                    (CallContextField::IsRoot, 1.into()),
                    (CallContextField::IsCreate, call.is_create().to_word()),
                    (CallContextField::CodeHash, callee_code_hash),
                ] {
                    state.call_context_write(&mut exec_step, call.call_id, field, value);
                }
            }
        }
    }

    exec_step.gas_cost = if geth_trace.struct_logs.is_empty() {
        GasCost(geth_trace.gas.0)
    } else {
        GasCost(state.tx.gas - geth_trace.struct_logs[0].gas.0)
    };

    // Initialize WASM global memory and global variables section
    for (i, byte) in geth_trace.global_memory.0.iter().enumerate() {
        // TODO: "I think there is easier way to proof init memory"
        state.memory_write(&mut exec_step, MemoryAddress::from(i), *byte)?;
    }
    for global in &geth_trace.globals {
        // TODO: "proof const evaluation"
        state.global_write(&mut exec_step, global.index, StackWord::from(global.value))?;
    }

    let first_function_call = geth_trace.function_calls.first().unwrap();
    // state.call_context_write(
    //     &mut exec_step,
    //     state.call()?.call_id,
    //     CallContextField::InternalFunctionId,
    //     U256::from(first_function_call.fn_index),
    // );

    for i in 0..first_function_call.num_locals {
        // TODO: "function body can be empty"
        state.stack_write(&mut exec_step, geth_trace.struct_logs[0].stack.nth_last_filled((first_function_call.num_locals - i - 1) as usize), StackWord::zero())?;
    }
    exec_step.function_index = first_function_call.fn_index;
    exec_step.max_stack_height = first_function_call.max_stack_height;
    exec_step.num_locals = first_function_call.num_locals;
    // increase reserved stack size with num locals
    exec_step.stack_size += first_function_call.num_locals as usize;

    let mut call_ctx = state.call_ctx_mut()?;
    call_ctx.memory = geth_trace.global_memory.clone();

    log::trace!("begin_tx_step: {:?}", exec_step);
    state.tx.steps_mut().push(exec_step);

    // TRICKY:
    // Process the reversion only for Precompile in begin TX. Since no associated
    // opcodes could process reversion afterwards.
    // TODO:
    // Move it to code of generating precompiled operations when implemented.
    if is_precompile && !state.call().unwrap().is_success {
        state.handle_reversion();
    }

    Ok(())
}

pub fn gen_end_tx_ops(state: &mut CircuitInputStateRef) -> Result<ExecStep, Error> {
    let mut exec_step = state.new_end_tx_step();
    let call = state.tx.calls()[0].clone();

    state.call_context_read(
        &mut exec_step,
        call.call_id,
        CallContextField::TxId,
        state.tx_ctx.id().into(),
    );
    state.call_context_read(
        &mut exec_step,
        call.call_id,
        CallContextField::IsPersistent,
        Word::from(call.is_persistent as u8),
    );

    let refund = state.sdb.refund();
    state.push_op(
        &mut exec_step,
        RW::READ,
        TxRefundOp {
            tx_id: state.tx_ctx.id(),
            value: refund,
            value_prev: refund,
        },
    );

    let effective_refund =
        refund.min((state.tx.gas - exec_step.gas_left.0) / MAX_REFUND_QUOTIENT_OF_GAS_USED as u64);
    let (found, caller_account) = state.sdb.get_account(&call.caller_address);
    if !found {
        return Err(Error::AccountNotFound(call.caller_address));
    }
    let caller_balance_prev = caller_account.balance;
    let caller_balance =
        caller_balance_prev + state.tx.gas_price * (exec_step.gas_left.0 + effective_refund);
    state.account_write(
        &mut exec_step,
        call.caller_address,
        AccountField::Balance,
        caller_balance,
        caller_balance_prev,
    )?;

    let block_info = state
        .block
        .headers
        .get(&state.tx.block_num)
        .unwrap()
        .clone();
    let effective_tip = state.tx.gas_price - block_info.base_fee;
    let gas_cost = state.tx.gas - exec_step.gas_left.0 - effective_refund;
    let coinbase_reward = effective_tip * gas_cost + state.tx_ctx.l1_fee;
    log::trace!(
        "coinbase reward = ({} - {}) * ({} - {} - {}) = {}",
        state.tx.gas_price,
        block_info.base_fee,
        state.tx.gas,
        exec_step.gas_left.0,
        effective_refund,
        coinbase_reward
    );
    let (found, coinbase_account) = state.sdb.get_account_mut(&block_info.coinbase);
    if !found {
        log::error!("coinbase account not found: {}", block_info.coinbase);
        return Err(Error::AccountNotFound(block_info.coinbase));
    }
    let coinbase_balance_prev = coinbase_account.balance;
    let coinbase_balance = coinbase_balance_prev + coinbase_reward;
    state.account_write(
        &mut exec_step,
        block_info.coinbase,
        AccountField::Balance,
        coinbase_balance,
        coinbase_balance_prev,
    )?;

    // handle tx receipt tag
    state.tx_receipt_write(
        &mut exec_step,
        state.tx_ctx.id(),
        TxReceiptField::PostStateOrStatus,
        call.is_persistent as u64,
    )?;

    let log_id = exec_step.log_id;
    state.tx_receipt_write(
        &mut exec_step,
        state.tx_ctx.id(),
        TxReceiptField::LogLength,
        log_id as u64,
    )?;

    if state.tx_ctx.id() > 1 {
        // query pre tx cumulative gas
        state.tx_receipt_read(
            &mut exec_step,
            state.tx_ctx.id() - 1,
            TxReceiptField::CumulativeGasUsed,
            state.block_ctx.cumulative_gas_used,
        )?;
    }

    state.block_ctx.cumulative_gas_used += state.tx.gas - exec_step.gas_left.0;
    state.tx_receipt_write(
        &mut exec_step,
        state.tx_ctx.id(),
        TxReceiptField::CumulativeGasUsed,
        state.block_ctx.cumulative_gas_used,
    )?;

    if !state.tx_ctx.is_last_tx() {
        state.call_context_write(
            &mut exec_step,
            state.block_ctx.rwc.0 + 1,
            CallContextField::TxId,
            (state.tx_ctx.id() + 1).into(),
        );
    }

    Ok(exec_step)
}

#[derive(Debug, Copy, Clone)]
struct DummySelfDestruct;

impl Opcode for DummySelfDestruct {
    fn gen_associated_ops(
        state: &mut CircuitInputStateRef,
        geth_steps: &[GethExecStep],
    ) -> Result<Vec<ExecStep>, Error> {
        dummy_gen_selfdestruct_ops(state, geth_steps)
    }
}
fn dummy_gen_selfdestruct_ops(
    state: &mut CircuitInputStateRef,
    geth_steps: &[GethExecStep],
) -> Result<Vec<ExecStep>, Error> {
    let geth_step = &geth_steps[0];
    let mut exec_step = state.new_step(geth_step)?;
    let sender = state.call()?.address;
    let receiver = geth_step.stack.last()?.to_address();

    let is_warm = state.sdb.check_account_in_access_list(&receiver);
    state.push_op_reversible(
        &mut exec_step,
        TxAccessListAccountOp {
            tx_id: state.tx_ctx.id(),
            address: receiver,
            is_warm: true,
            is_warm_prev: is_warm,
        },
    )?;

    let (found, receiver_account) = state.sdb.get_account(&receiver);
    if !found {
        return Err(Error::AccountNotFound(receiver));
    }
    let receiver_account = &receiver_account.clone();
    let (found, sender_account) = state.sdb.get_account(&sender);
    if !found {
        return Err(Error::AccountNotFound(sender));
    }
    let sender_account = &sender_account.clone();
    let value = sender_account.balance;
    log::trace!(
        "self destruct, sender {:?} receiver {:?} value {:?}",
        sender,
        receiver,
        value
    );
    // NOTE: In this dummy implementation we assume that the receiver already
    // exists.

    state.push_op_reversible(
        &mut exec_step,
        AccountOp {
            address: sender,
            field: AccountField::Balance,
            value: Word::zero(),
            value_prev: value,
        },
    )?;
    state.push_op_reversible(
        &mut exec_step,
        AccountOp {
            address: sender,
            field: AccountField::Nonce,
            value: Word::zero(),
            value_prev: sender_account.nonce,
        },
    )?;
    state.push_op_reversible(
        &mut exec_step,
        AccountOp {
            address: sender,
            field: AccountField::CodeHash,
            value: Word::zero(),
            value_prev: sender_account.code_hash.to_word(),
        },
    )?;
    if receiver != sender {
        state.push_op_reversible(
            &mut exec_step,
            AccountOp {
                address: receiver,
                field: AccountField::Balance,
                value: receiver_account.balance + value,
                value_prev: receiver_account.balance,
            },
        )?;
    }

    if state.call()?.is_persistent {
        state.sdb.destruct_account(sender);
    }

    state.handle_return(&mut exec_step, geth_steps, false)?;
    Ok(vec![exec_step])
}
