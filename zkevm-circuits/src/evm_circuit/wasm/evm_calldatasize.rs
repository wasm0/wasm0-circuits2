use halo2_proofs::circuit::Value;
use crate::{
    evm_circuit::{
        execution::ExecutionGadget,
        param::N_BYTES_CALLDATASIZE,
        step::ExecutionState,
        util::{
            common_gadget::SameContextGadget,
            constraint_builder::{StepStateTransition, Transition::Delta},
            from_bytes, CachedRegion, RandomLinearCombination,
        },
        witness::{Block, Call, ExecStep, Transaction},
    },
    table::CallContextFieldTag,
    util::Expr,
};
use bus_mapping::evm::OpcodeId;
use eth_types::{Field, ToLittleEndian};
use halo2_proofs::plonk::Error;
use crate::evm_circuit::util::Cell;
use crate::evm_circuit::util::constraint_builder::EVMConstraintBuilder;

#[derive(Clone, Debug)]
pub(crate) struct EvmCallDataSizeGadget<F> {
    same_context: SameContextGadget<F>,
    call_data_size: RandomLinearCombination<F, N_BYTES_CALLDATASIZE>,
    dest: Cell<F>,
}

impl<F: Field> ExecutionGadget<F> for EvmCallDataSizeGadget<F> {
    const NAME: &'static str = "CALLDATASIZE";

    const EXECUTION_STATE: ExecutionState = ExecutionState::CALLDATASIZE;

    fn configure(cb: &mut EVMConstraintBuilder<F>) -> Self {
        let opcode = cb.query_cell();
        let dest = cb.query_cell();

        // Add lookup constraint in the call context for the calldatasize field.
        let call_data_size = cb.query_word_rlc();
        cb.call_context_lookup(
            false.expr(),
            None,
            CallContextFieldTag::CallDataLength,
            from_bytes::expr(&call_data_size.cells),
        );

        // The calldatasize should be pushed to the top of the stack.
        cb.stack_pop(dest.expr());

        let step_state_transition = StepStateTransition {
            rw_counter: Delta(2.expr()),
            program_counter: Delta(1.expr()),
            stack_pointer: Delta((-1).expr()),
            gas_left: Delta(-OpcodeId::CALLDATASIZE.constant_gas_cost().expr()),
            ..Default::default()
        };

        let same_context = SameContextGadget::construct(cb, opcode, step_state_transition);

        cb.memory_rlc_lookup(1.expr(), &dest, &call_data_size);

        Self {
            same_context,
            call_data_size,
            dest,
        }
    }

    fn assign_exec_step(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        block: &Block<F>,
        _tx: &Transaction,
        _call: &Call,
        step: &ExecStep,
    ) -> Result<(), Error> {
        self.same_context.assign_exec_step(region, offset, step)?;

        let call_data_size = block.rws[step.rw_indices[0]].call_context_value();
        let dest_offset = block.rws[step.rw_indices[1]].stack_value();

        self.call_data_size.assign(
            region,
            offset,
            Some(
                call_data_size.to_le_bytes()[..N_BYTES_CALLDATASIZE]
                    .try_into()
                    .unwrap(),
            ),
        )?;
        self.dest.assign(
            region,
            offset,
            Value::known(F::from(dest_offset.as_u64())),
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::{evm_circuit::test::rand_bytes, test_util::CircuitTestBuilder};
    use bus_mapping::circuit_input_builder::CircuitsParams;
    use eth_types::{address, bytecode, Bytecode, bytecode_internal, Word};

    use itertools::Itertools;
    use eth_types::bytecode::WasmBinaryBytecode;
    use mock::TestContext;

    fn test_ok(call_data_size: usize, is_root: bool) {
        let mut bytecode = Bytecode::default();
        let dest_offset = bytecode.alloc_default_global_data(8);
        bytecode_internal! {bytecode,
            I32Const[dest_offset]
            CALLDATASIZE
        };

        if is_root {
            let ctx = TestContext::<2, 1>::new(
                None,
                |accs| {
                    accs[0]
                        .address(address!("0x0000000000000000000000000000000000000123"))
                        .balance(Word::from(1u64 << 30));
                    accs[1]
                        .address(address!("0x0000000000000000000000000000000000000010"))
                        .balance(Word::from(1u64 << 20))
                        .code(bytecode.wasm_binary());
                },
                |mut txs, accs| {
                    txs[0]
                        .from(accs[0].address)
                        .to(accs[1].address)
                        .input(rand_bytes(call_data_size).into())
                        .gas(Word::from(40000));
                },
                |block, _tx| block.number(0xcafeu64),
            )
            .unwrap();

            CircuitTestBuilder::new_from_test_ctx(ctx)
                .params(CircuitsParams {
                    max_calldata: 1200,
                    ..CircuitsParams::default()
                })
                .run();
        } else {
            let ctx = TestContext::<3, 1>::new(
                None,
                |accs| {
                    accs[0]
                        .address(address!("0x0000000000000000000000000000000000000123"))
                        .balance(Word::from(1u64 << 30));
                    accs[1]
                        .address(address!("0x0000000000000000000000000000000000000010"))
                        .balance(Word::from(1u64 << 20))
                        .code(bytecode! {
                            PUSH1(0)
                            PUSH1(0)
                            PUSH32(call_data_size)
                            PUSH1(0)
                            PUSH1(0)
                            PUSH1(0x20)
                            GAS
                            CALL
                            STOP
                        });
                    accs[2]
                        .address(address!("0x0000000000000000000000000000000000000020"))
                        .balance(Word::from(1u64 << 20))
                        .code(bytecode.wasm_binary());
                },
                |mut txs, accs| {
                    txs[0]
                        .from(accs[0].address)
                        .to(accs[1].address)
                        .gas(Word::from(30000));
                },
                |block, _tx| block.number(0xcafeu64),
            )
            .unwrap();

            CircuitTestBuilder::new_from_test_ctx(ctx)
                .params(CircuitsParams {
                    max_calldata: 600,
                    ..CircuitsParams::default()
                })
                .run();
        };
    }

    #[test]
    fn calldatasize_gadget_root() {
        for (call_data_size, is_root) in vec![32, 64, 96, 128, 256, 512, 1024]
            .into_iter()
            .cartesian_product([true])
        {
            test_ok(call_data_size, is_root);
        }
    }
}
