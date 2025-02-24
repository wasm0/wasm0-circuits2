use eth_types::{GethExecStep, ToBigEndian, ToLittleEndian};
use eth_types::evm_types::MemoryAddress;

use crate::circuit_input_builder::{CircuitInputStateRef, ExecStep};
use crate::Error;

use super::Opcode;

pub(crate) const STACK_TO_MEMORY_TYPE_DEFAULT: usize = 0;
pub(crate) const STACK_TO_MEMORY_TYPE_U256: usize = 32;
pub(crate) const STACK_TO_MEMORY_TYPE_U64: usize = 8;

/// Placeholder structure used to implement [`Opcode`] trait over it
/// corresponding to all the Stack only operations: take N words and return one.
/// The following cases exist in the EVM:
/// - N = 1: UnaryOpcode
/// - N = 2: BinaryOpcode
/// - N = 3: TernaryOpcode
#[derive(Debug, Copy, Clone)]
pub(crate) struct StackToMemoryOpcode<
    const N_POP: usize,
    const EL_TYPE: usize = { STACK_TO_MEMORY_TYPE_DEFAULT },
>;

impl<const N_POP: usize, const N_BYTES: usize> Opcode for StackToMemoryOpcode<N_POP, N_BYTES> {
    fn gen_associated_ops(
        state: &mut CircuitInputStateRef,
        geth_steps: &[GethExecStep],
    ) -> Result<Vec<ExecStep>, Error> {
        let geth_step = &geth_steps[0];
        let mut exec_step = state.new_step(geth_step)?;

        // Read dest offset as the last stack element
        let dest_offset = geth_step.stack.nth_last(0)?;
        state.stack_read(&mut exec_step, geth_step.stack.nth_last_filled(0), dest_offset)?;
        let offset_addr = MemoryAddress::try_from(dest_offset)?;

        // Pop elements from stack
        for i in 0..N_POP {
            state.stack_read(
                &mut exec_step,
                geth_step.stack.nth_last_filled(i + 1),
                geth_step.stack.nth_last(i + 1)?,
            )?;
        }

        // Copy result to memory
        let value = if N_BYTES == STACK_TO_MEMORY_TYPE_DEFAULT {
            geth_steps[1].memory[0].0.clone()
        } else if N_BYTES == STACK_TO_MEMORY_TYPE_U256 {
            geth_steps[1].global_memory.read_u256(dest_offset)?.to_be_bytes().to_vec()
        } else if N_BYTES == STACK_TO_MEMORY_TYPE_U64 {
            geth_steps[1].global_memory.read_u64(dest_offset)?.to_be_bytes().to_vec()
        } else {
            unreachable!("not possible EL_TYPE specified");
        };
        let it = if N_BYTES > 0 {
            value.iter().skip(32 - N_BYTES)
        } else {
            value.iter().skip(0)
        };

        for (i, b) in it.enumerate() {
            state.memory_write(&mut exec_step, offset_addr.map(|a| a + i), *b)?;
        }
        let call_ctx = state.call_ctx_mut()?;
        call_ctx.memory = geth_steps[1].global_memory.clone();

        Ok(vec![exec_step])
    }
}

#[cfg(test)]
mod stacktomemoryop_tests {
    use itertools::Itertools;
    use pretty_assertions::assert_eq;

    use eth_types::{bytecode, Bytecode, evm_types::{OpcodeId, StackAddress}, geth_types::GethData, StackWord, ToBigEndian, ToLittleEndian, Word};
    use mock::{MOCK_BASEFEE, MOCK_DIFFICULTY, MOCK_GASLIMIT};
    use mock::test_ctx::{helpers::*, TestContext};

    use crate::{circuit_input_builder::ExecState, mock::BlockData, operation::StackOp};
    use crate::operation::RW;

    fn stack_to_memory_op_impl<const N_POP: usize, const N_PUSH: usize>(
        opcode: OpcodeId,
        code: Bytecode,
        pops: Vec<StackOp>,
        pushes: Vec<StackOp>,
        mem: Vec<u8>,
    ) {
        // Get the execution steps from the external tracer
        let block: GethData = TestContext::<2, 1>::new(
            None,
            account_0_code_account_1_no_code(code),
            tx_from_1_to_0,
            |block, _tx| block.number(0xcafeu64),
        )
            .unwrap()
            .into();

        let mut builder = BlockData::new_from_geth_data(block.clone()).new_circuit_input_builder();
        builder
            .handle_block(&block.eth_block, &block.geth_traces)
            .unwrap();

        let step = builder.block.txs()[0]
            .steps()
            .iter()
            .find(|step| step.exec_state == ExecState::Op(opcode))
            .unwrap();

        assert_eq!(
            (0..N_POP)
                .map(|idx| {
                    &builder.block.container.stack[step.bus_mapping_instance[idx].as_usize()]
                })
                .map(|operation| (operation.rw(), operation.op().clone()))
                .collect_vec(),
            pops.into_iter().map(|pop| (RW::READ, pop)).collect_vec()
        );
        assert_eq!(
            (0..N_PUSH)
                .map(|idx| {
                    &builder.block.container.stack
                        [step.bus_mapping_instance[N_POP + idx].as_usize()]
                })
                .map(|operation| (operation.rw(), operation.op().clone()))
                .collect_vec(),
            pushes
                .into_iter()
                .map(|push| (RW::WRITE, push))
                .collect_vec()
        );

        let memory = (0..mem.len())
            .map(|idx| {
                &builder.block.container.memory[step.bus_mapping_instance[N_POP + N_PUSH + idx].as_usize()]
            })
            .map(|operation| operation.op().value())
            .collect_vec();
        assert_eq!(memory, mem);
    }

    #[test]
    fn difficulty_opcode_impl() {
        stack_to_memory_op_impl::<1, 0>(
            OpcodeId::DIFFICULTY,
            bytecode! {
                I32Const[0]
                DIFFICULTY
                STOP
            },
            vec![StackOp::new(1, StackAddress(1023), StackWord::from(0))],
            vec![],
            Vec::from(MOCK_DIFFICULTY.to_be_bytes()),
        );
    }

    #[test]
    fn gas_limit_opcode_impl() {
        stack_to_memory_op_impl::<1, 0>(
            OpcodeId::GASLIMIT,
            bytecode! {
                I32Const[0]
                GASLIMIT
                STOP
            },
            vec![StackOp::new(1, StackAddress(1023), StackWord::from(0))],
            vec![],
            Vec::from(MOCK_GASLIMIT.as_u64().to_be_bytes()),
        );
    }

    #[test]
    fn basefee_opcode_impl() {
        stack_to_memory_op_impl::<1, 0>(
            OpcodeId::BASEFEE,
            bytecode! {
                I32Const[0]
                BASEFEE
                STOP
            },
            vec![StackOp::new(1, StackAddress(1023), StackWord::from(0))],
            vec![],
            Vec::from(MOCK_BASEFEE.to_be_bytes()),
        );
    }
}
