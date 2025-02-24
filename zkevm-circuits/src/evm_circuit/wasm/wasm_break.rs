use halo2_proofs::plonk::Error;

use bus_mapping::evm::OpcodeId;
use eth_types::Field;

use crate::{
    evm_circuit::{
        execution::ExecutionGadget,
        step::ExecutionState,
        util::{
            CachedRegion,
            common_gadget::SameContextGadget,
            constraint_builder::{StepStateTransition, Transition::To, Transition::Delta},
        },
        witness::{Block, Call, ExecStep, Transaction},
    },
    util::Expr,
};
use crate::evm_circuit::util::Cell;
use crate::evm_circuit::util::constraint_builder::EVMConstraintBuilder;

#[derive(Clone, Debug)]
pub(crate) struct WasmBreakGadget<F> {
    same_context: SameContextGadget<F>,
    program_counter: Cell<F>,
}

impl<F: Field> ExecutionGadget<F> for WasmBreakGadget<F> {
    const NAME: &'static str = "WASM_BREAK";

    const EXECUTION_STATE: ExecutionState = ExecutionState::WASM_BREAK;

    fn configure(cb: &mut EVMConstraintBuilder<F>) -> Self {
        let program_counter = cb.query_cell();

        let step_state_transition = StepStateTransition {
            rw_counter: Delta(2.expr()),
            program_counter: To(program_counter.expr()),
            stack_pointer: Delta(0.expr()),
            gas_left: Delta(-OpcodeId::Call.constant_gas_cost().expr()),
            ..Default::default()
        };

        let opcode = cb.query_cell();
        let same_context = SameContextGadget::construct(cb, opcode, step_state_transition);

        Self {
            same_context,
            program_counter,
        }
    }

    fn assign_exec_step(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        _block: &Block<F>,
        _: &Transaction,
        _call: &Call,
        step: &ExecStep,
    ) -> Result<(), Error> {
        self.same_context.assign_exec_step(region, offset, step)?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use wasm_encoder::ValType;
    use eth_types::{bytecode, Bytecode};
    use mock::test_ctx::TestContext;

    use crate::test_util::CircuitTestBuilder;

    fn run_test(bytecode: Bytecode) {
        CircuitTestBuilder::new_from_test_ctx(
            TestContext::<2, 1>::simple_ctx_with_bytecode(bytecode).unwrap(),
        ).run()
    }

    // #[test]
    // fn test_wasm_locals_encoding() {
    //     let mut code = bytecode! {
    //         I32Const[100]
    //         I32Const[20]
    //         Call[0]
    //         Drop
    //     };
    //     code.new_function(vec![ValType::I32; 2], vec![ValType::I32; 1], bytecode! {
    //         GetLocal[0]
    //         GetLocal[1]
    //         I32Add
    //         SetLocal[2]
    //         I32Const[0]
    //         TeeLocal[2]
    //         Return
    //     }, vec![(1, ValType::I32)]);
    //     run_test(code);
    // }

    #[test]
    fn test_wasm_br_breaks_1() {
        let code = bytecode! {
            Block
                I32Const[1]
                I32Const[2]
                I32Add
                Br[0]
                I32Const[100]
                Drop
            End
        };
        run_test(code);
    }

    #[test]
    fn test_wasm_br_if_breaks_1() {
        let code = bytecode! {
            Block
                I32Const[1]
                I32Const[2]
                I32Add
                BrIf[0]
                I32Const[100]
                Drop
            End
        };
        run_test(code);
    }

    #[test]
    fn test_wasm_br_if_breaks_to_lv2_1() {
        let code = bytecode! {
            Block
                Block
                    I32Const[1]
                    I32Const[2]
                    I32Add
                    BrIf[1]
                    I32Const[100]
                    Drop
                End
            End
        };
        run_test(code);
    }

    #[test]
    fn test_wasm_br_if_loop_breaks_to_lv2_1() {
        let code = bytecode! {
            Block
                Loop
                    I32Const[1]
                    I32Const[2]
                    I32Add
                    BrIf[1]
                    I32Const[100]
                    Drop
                End
            End
        };
        run_test(code);
    }

    #[test]
    fn test_wasm_br_if_loop_breaks_to_lv2_on_2nd_iteration_1() {
        let mut code = bytecode! {
            I32Const[0]
            SetLocal[0]
            Block
                Loop
                    GetLocal[0]
                    BrIf[1]
                    GetLocal[0]
                    I32Const[1]
                    I32Add
                    SetLocal[0]
                    Br[0]
                End
            End
        };
        code.with_main_locals(vec![(1, ValType::I32)]);
        run_test(code);
    }

    #[test]
    fn test_wasm_br_if_no_breaks_1() {
        let code = bytecode! {
            Block
                I32Const[0]
                I32Const[0]
                I32Add
                BrIf[0]
                I32Const[100]
                Drop
            End
        };
        run_test(code);
    }
}
