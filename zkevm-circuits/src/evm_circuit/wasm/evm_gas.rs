use array_init::array_init;
use halo2_proofs::circuit::Value;
use crate::{
    evm_circuit::{
        execution::ExecutionGadget,
        param::N_BYTES_GAS,
        step::ExecutionState,
        util::{
            common_gadget::SameContextGadget,
            constraint_builder::{ConstrainBuilderCommon, StepStateTransition, Transition::Delta},
            from_bytes, CachedRegion, RandomLinearCombination,
        },
        witness::{Block, Call, ExecStep, Transaction},
    },
    util::Expr,
};
use eth_types::{evm_types::OpcodeId, Field};
use halo2_proofs::plonk::Error;
use crate::evm_circuit::param::N_BYTES_U64;
use crate::evm_circuit::util::Cell;
use crate::evm_circuit::util::constraint_builder::EVMConstraintBuilder;
use crate::evm_circuit::util::math_gadget::BinaryNumberGadget;
use crate::evm_circuit::util::memory_gadget::MemoryExpansionGadget;

#[derive(Clone, Debug)]
pub(crate) struct EvmGasGadget<F> {
    same_context: SameContextGadget<F>,
    gas_left: [Cell<F>; N_BYTES_U64],
    dest_offset: Cell<F>,
}

impl<F: Field> ExecutionGadget<F> for EvmGasGadget<F> {
    const NAME: &'static str = "GAS";

    const EXECUTION_STATE: ExecutionState = ExecutionState::GAS;

    fn configure(cb: &mut EVMConstraintBuilder<F>) -> Self {
        // The gas passed to a transaction is a 64-bit number.
        let gas_left: [Cell<F>; N_BYTES_U64] = array_init(|_| cb.query_cell());
        let dest_offset = cb.query_cell();

        cb.stack_pop(dest_offset.expr());
        cb.memory_array_lookup(1.expr(), &dest_offset, &gas_left);

        // The `gas_left` in the current state has to be deducted by the gas
        // used by the `GAS` opcode itself.
        cb.require_equal(
            "Constraint: gas left equal to stack value",
            from_bytes::expr(&gas_left),
            cb.curr.state.gas_left.expr() - OpcodeId::GAS.constant_gas_cost().expr(),
        );

        let step_state_transition = StepStateTransition {
            rw_counter: Delta(1.expr()),
            program_counter: Delta(1.expr()),
            stack_pointer: Delta((-1).expr()),
            gas_left: Delta(-OpcodeId::GAS.constant_gas_cost().expr()),
            ..Default::default()
        };
        let opcode = cb.query_cell();
        let same_context = SameContextGadget::construct(cb, opcode, step_state_transition);

        Self {
            same_context,
            gas_left,
            dest_offset,
        }
    }

    fn assign_exec_step(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        block: &Block<F>,
        _transaction: &Transaction,
        _call: &Call,
        step: &ExecStep,
    ) -> Result<(), Error> {
        self.same_context.assign_exec_step(region, offset, step)?;

        let dest_offset = block.rws[step.rw_indices[0]].stack_value();
        self.dest_offset.assign(region, offset, Value::known(F::from(dest_offset.as_u64())))?;

        // The GAS opcode takes into account the reduction of gas available due
        // to the instruction itself.
        let gas_left = step.gas_left.saturating_sub(OpcodeId::GAS.constant_gas_cost().as_u64());
        let gas_left = gas_left.to_le_bytes();
        for i in 0..N_BYTES_U64 {
            self.gas_left[i].assign(region, offset, Value::known(F::from(gas_left[i] as u64)))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::test_util::CircuitTestBuilder;
    use eth_types::{address, bytecode, Bytecode, bytecode_internal, Word};
    use eth_types::bytecode::WasmBinaryBytecode;
    use mock::TestContext;

    fn test_ok() {
        let mut code = Bytecode::default();
        let dest = code.alloc_default_global_data(8);

        bytecode_internal! {code,
            I32Const[dest]
            GAS
        };

        CircuitTestBuilder::new_from_test_ctx(
            TestContext::<2, 1>::simple_ctx_with_bytecode(code).unwrap(),
        )
        .run();
    }

    #[test]
    fn gas_gadget_simple() {
        test_ok();
    }

    #[test]
    fn gas_gadget_incorrect_deduction() {
        let mut code = Bytecode::default();
        let dest = code.alloc_default_global_data(8);

        bytecode_internal! {code,
            I32Const[dest]
            GAS
        };

        let bytecode = code.wasm_binary();

        // Create a custom tx setting Gas to
        let ctx = TestContext::<2, 1>::new(
            None,
            |accs| {
                accs[0]
                    .address(address!("0x0000000000000000000000000000000000000010"))
                    .balance(Word::from(1u64 << 20))
                    .code(bytecode);
                accs[1]
                    .address(address!("0x0000000000000000000000000000000000000000"))
                    .balance(Word::from(1u64 << 20));
            },
            |mut txs, accs| {
                txs[0]
                    .to(accs[0].address)
                    .from(accs[1].address)
                    .gas(Word::from(1_000_000u64));
            },
            |block, _tx| block.number(0xcafeu64),
        )
        .unwrap();

        CircuitTestBuilder::<2, 1>::new_from_test_ctx(ctx)
            .block_modifier(Box::new(|block| {
                // The above block has 2 steps (GAS and STOP). We forcefully assign a
                // wrong `gas_left` value for the second step, to assert that
                // the circuit verification fails for this scenario.
                assert_eq!(block.txs.len(), 1);
                assert_eq!(block.txs[0].steps.len(), 6);
                block.txs[0].steps[3].gas_left -= 1;
            }))
            .evm_checks(Box::new(|prover, gate_rows, lookup_rows| {
                assert!(prover
                    .verify_at_rows_par(gate_rows.iter().cloned(), lookup_rows.iter().cloned())
                    .is_err())
            }))
            .run();
    }
}
