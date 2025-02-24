use halo2_proofs::circuit::Value;
use crate::{
    evm_circuit::{
        execution::ExecutionGadget,
        step::ExecutionState,
        util::{
            common_gadget::SameContextGadget,
            constraint_builder::{EVMConstraintBuilder, StepStateTransition, Transition::Delta},
            CachedRegion, Cell,
        },
        witness::{Block, Call, ExecStep, Transaction},
    },
    table::BlockContextFieldTag,
    util::Expr,
};
use bus_mapping::evm::OpcodeId;
use eth_types::{Field, N_BYTES_WORD, ToLittleEndian, Word};
use halo2_proofs::plonk::Error;
use itertools::Itertools;
use crate::evm_circuit::util::RandomLinearCombination;

#[derive(Clone, Debug)]
pub(crate) struct EvmChainIdGadget<F> {
    same_context: SameContextGadget<F>,
    chain_id: RandomLinearCombination<F, 32>,
    dest_offset: Cell<F>,
}

impl<F: Field> ExecutionGadget<F> for EvmChainIdGadget<F> {
    const NAME: &'static str = "CHAINID";

    const EXECUTION_STATE: ExecutionState = ExecutionState::CHAINID;

    fn configure(cb: &mut EVMConstraintBuilder<F>) -> Self {
        let chain_id = cb.query_word_rlc();
        let dest_offset = cb.query_cell();

        cb.stack_pop(dest_offset.expr());

        // Lookup block table with chain_id
        cb.block_lookup(
            BlockContextFieldTag::ChainId.expr(),
            cb.curr.state.block_number.expr(),
            chain_id.expr(),
        );
        cb.memory_rlc_lookup(true.expr(), &dest_offset, &chain_id);

        // State transition
        let opcode = cb.query_cell();
        let step_state_transition = StepStateTransition {
            rw_counter: Delta(1.expr()),
            program_counter: Delta(1.expr()),
            stack_pointer: Delta((-1).expr()),
            gas_left: Delta(-OpcodeId::CHAINID.constant_gas_cost().expr()),
            ..Default::default()
        };
        let same_context = SameContextGadget::construct(cb, opcode, step_state_transition);

        Self {
            same_context,
            chain_id,
            dest_offset,
        }
    }

    fn assign_exec_step(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        block: &Block<F>,
        _: &Transaction,
        _: &Call,
        step: &ExecStep,
    ) -> Result<(), Error> {
        self.same_context.assign_exec_step(region, offset, step)?;

        let dest_offset = block.rws[step.rw_indices[0]].stack_value();
        let chain_bytes = (1..=32).map(|i| block.rws[step.rw_indices[i]].memory_value())
            .collect_vec();
        let chain_id = Word::from_big_endian(chain_bytes.as_slice());

        self.dest_offset.assign(region, offset, Value::known(F::from(dest_offset.as_u64())))?;
        self.chain_id.assign(
            region,
            offset,
            Some(chain_id.to_le_bytes()[0..N_BYTES_WORD].try_into().unwrap()),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::test_util::CircuitTestBuilder;
    use eth_types::bytecode;
    use mock::test_ctx::TestContext;

    #[test]
    fn chainid_gadget_test() {
        let bytecode = bytecode! {
            I32Const[0x7f]
            CHAINID
        };

        CircuitTestBuilder::new_from_test_ctx(
            TestContext::<2, 1>::simple_ctx_with_bytecode(bytecode).unwrap(),
        ).run();
    }
}
