use halo2_proofs::circuit::Value;
use halo2_proofs::plonk::{Error, Expression};

use bus_mapping::evm::OpcodeId;
use eth_types::{Field, ToScalar};

use crate::{
    evm_circuit::{
        execution::ExecutionGadget,
        step::ExecutionState,
        util::{
            CachedRegion,
            common_gadget::SameContextGadget,
            constraint_builder::{ConstrainBuilderCommon, StepStateTransition, Transition::Delta},
        },
        witness::{Block, Call, ExecStep, Transaction},
    },
    util::Expr,
};
use crate::evm_circuit::util::Cell;

#[derive(Clone, Debug)]
pub(crate) struct WasmStoreGadget<F> {
    same_context: SameContextGadget<F>,

    opcode_store_offset: Cell<F>,

    store_start_block_index: Cell<F>,
    store_start_block_inner_offset: Cell<F>,
    store_start_block_inner_offset_helper: Cell<F>,

    store_end_block_index: Cell<F>,
    store_end_block_inner_offset: Cell<F>,
    store_end_block_inner_offset_helper: Cell<F>,

    load_value1: Cell<F>,
    load_value2: Cell<F>,
    store_value1: Cell<F>,
    store_value2: Cell<F>,

    mask_bits: [Cell<F>; 16],
    offset_modulus: Cell<F>,
    store_raw_value: Cell<F>,
    store_base: Cell<F>,
    store_wrapped_value: Cell<F>,

    vtype: Cell<F>,
    is_one_byte: Cell<F>,
    is_two_bytes: Cell<F>,
    is_four_bytes: Cell<F>,
    is_eight_bytes: Cell<F>,

    //lookup_offset_len_bits: OffsetLenBitsTableLookupCell,
    //lookup_pow: PowTableLookupCell,

    address_within_allocated_pages_helper: Cell<F>,
}

impl<F: Field> ExecutionGadget<F> for WasmStoreGadget<F> {
    const NAME: &'static str = "WASM_STORE";

    const EXECUTION_STATE: ExecutionState = ExecutionState::WASM_STORE;

    fn configure(cb: &mut ConstrainBuilderCommon<F>) -> Self {
        let opcode_store_offset = cb.alloc_common_range_value();

        let store_start_block_index = cb.alloc_common_range_value();
        let store_start_block_inner_offset = cb.alloc_common_range_value();
        let store_start_block_inner_offset_helper = cb.alloc_common_range_value();

        let store_end_block_index = cb.alloc_common_range_value();
        let store_end_block_inner_offset = cb.alloc_common_range_value();
        let store_end_block_inner_offset_helper = cb.alloc_common_range_value();

        let load_value1 = cb.alloc_u64_on_u8();
        let load_value2 = cb.alloc_u64_on_u8();
        let store_value1 = cb.alloc_u64_on_u8();
        let store_value2 = cb.alloc_u64_on_u8();
        let offset_modulus = cb.alloc_u64();
        let store_raw_value = cb.alloc_u64();
        let store_base = cb.alloc_u64();

        let store_wrapped_value = cb.alloc_unlimited_value();

        let mask_bits = [0; 16].map(|_| cb.alloc_bit_value());
        let is_one_byte = cb.alloc_bit_value();
        let is_two_bytes = cb.alloc_bit_value();
        let is_four_bytes = cb.alloc_bit_value();
        let is_eight_bytes = cb.alloc_bit_value();
        let vtype = cb.alloc_common_range_value();

        let lookup_offset_len_bits = cb.alloc_offset_len_bits_table_lookup();
        let lookup_pow = cb.alloc_pow_table_lookup();

        let current_memory_page_size = cb.allocated_memory_pages_cell();
        let address_within_allocated_pages_helper = cb.alloc_common_range_value();

        cb.stack_pop(value.expr());
        cb.stack_pop(raw_address.expr());
        cb.stack_pop(pre_block_value.expr());
        cb.stack_push(update_block_value1.expr());

        cb.require_zeros("op_store: start end offset range", vec![
            store_start_block_inner_offset.expr()
                + store_start_block_inner_offset_helper.expr()
                - 7.expr(),
            store_end_block_inner_offset.expr()
                + store_end_block_inner_offset_helper.expr()
                - 7.expr(),
        ]);

        cb.require_zeros("op_store: start end equation", {
            let len = 1.expr()
                + is_two_bytes.expr() * 1.expr()
                + is_four_bytes.expr() * 3.expr()
                + is_eight_bytes.expr() * 7.expr();
            vec![
                store_start_block_index.expr() * 8.expr()
                    + store_start_block_inner_offset.expr()
                    + len
                    - 1.expr()
                    - store_end_block_index.expr() * 8.expr()
                    - store_end_block_inner_offset.expr(),
            ]
        });

        cb.require_zeros("op_store: start store_base", vec![
            store_base.expr() + opcode_store_offset.expr()
                - store_start_block_index.expr() * 8.expr()
                - store_start_block_inner_offset.expr(),
        ]);

        cb.require_zeros("op_store: length", vec![
            is_one_byte.expr()
                + is_two_bytes.expr()
                + is_four_bytes.expr()
                + is_eight_bytes.expr()
                - 1.expr(),
        ]);

        cb.require_zeros("op_store: mask_bits offset len", {
            let len = 1.expr()
                + is_two_bytes.expr() * 1.expr()
                + is_four_bytes.expr() * 3.expr()
                + is_eight_bytes.expr() * 7.expr();
            let (_, bits_encode) = mask_bits
                .map(|c| c.expr())
                .into_iter()
                .enumerate()
                .reduce(|(_, acc), (i, e)| (i, acc + e * (1u64 << i).expr()))
                .unwrap();
            vec![
                lookup_offset_len_bits.expr()
                    - offset_len_bits_encode_expr(
                        store_start_block_inner_offset.expr(),
                        len,
                        bits_encode,
                    ),
            ]
        });

        cb.require_zeros("op_store: pow table lookup", vec![
            lookup_pow.expr()
                - pow_table_encode(
                    offset_modulus.expr(),
                    store_start_block_inner_offset.expr() * 8.expr(),
                ),
        ]);

        /*constraint_builder.push(
            "op_store wrap value",
            Box::new(move |meta| {
                let has_two_bytes =
                    is_two_bytes.expr(meta) + is_four_bytes.expr(meta) + is_eight_bytes.expr(meta);
                let has_four_bytes = is_four_bytes.expr(meta) + is_eight_bytes.expr(meta);
                let has_eight_bytes = is_eight_bytes.expr(meta);
                let byte_value = (0..8)
                    .map(|i| {
                        store_raw_value.u4_expr(meta, i * 2) * constant_from!(1u64 << (8 * i))
                            + store_raw_value.u4_expr(meta, i * 2 + 1)
                                * constant_from!(1u64 << (8 * i + 4))
                    })
                    .collect::<Vec<_>>();
                vec![
                    byte_value[0].clone()
                        + byte_value[1].clone() * has_two_bytes
                        + (byte_value[2].clone() + byte_value[3].clone()) * has_four_bytes
                        + (byte_value[4].clone()
                            + byte_value[5].clone()
                            + byte_value[6].clone()
                            + byte_value[7].clone())
                            * has_eight_bytes
                        - store_wrapped_value.expr(meta),
                ]
            }),
        );*/

        /*constraint_builder.push(
            "op_store write value",
            Box::new(move |meta| {
                let mut acc = store_wrapped_value.expr(meta) * offset_modulus.expr(meta);

                for i in 0..8 {
                    acc = acc
                        - store_value1.u8_expr(meta, i)
                            * constant!(bn_to_field(&(BigUint::from(1u64) << (i * 8))))
                            * mask_bits[i as usize].expr(meta);

                    acc = acc
                        - store_value2.u8_expr(meta, i)
                            * constant!(bn_to_field(&(BigUint::from(1u64) << (i * 8 + 64))))
                            * mask_bits[i as usize + 8].expr(meta);
                }

                vec![acc]
            }),
        );*/

        /*constraint_builder.push(
            "op_store unchanged value",
            Box::new(move |meta| {
                let mut acc = constant_from!(0);

                for i in 0..8 {
                    acc = acc
                        + load_value1.u8_expr(meta, i)
                            * constant!(bn_to_field(&(BigUint::from(1u64) << (i * 8))))
                            * (constant_from!(1) - mask_bits[i as usize].expr(meta))
                        - store_value1.u8_expr(meta, i)
                            * constant!(bn_to_field(&(BigUint::from(1u64) << (i * 8))))
                            * (constant_from!(1) - mask_bits[i as usize].expr(meta));

                    acc = acc
                        + load_value2.u8_expr(meta, i)
                            * constant!(bn_to_field(&(BigUint::from(1u64) << (i * 8 + 64))))
                            * (constant_from!(1) - mask_bits[i as usize + 8].expr(meta))
                        - store_value2.u8_expr(meta, i)
                            * constant!(bn_to_field(&(BigUint::from(1u64) << (i * 8 + 64))))
                            * (constant_from!(1) - mask_bits[i as usize + 8].expr(meta));
                }

                vec![acc]
            }),
        );*/

        cb.require_zeros("op_store: allocated address", {
            let len = 1.expr()
                + is_two_bytes.expr() * 1.expr()
                + is_four_bytes.expr() * 3.expr()
                + is_eight_bytes.expr() * 7.expr();
            vec![
                (store_base.expr()
                    + opcode_store_offset.expr()
                    + len
                    + address_within_allocated_pages_helper.expr()
                    - current_memory_page_size.expr() * WASM_PAGE_SIZE.expr()),
            ]
        });

        let opcode = cb.query_cell();

        // State transition
        let step_state_transition = StepStateTransition {
            rw_counter: Delta(4.expr()),
            program_counter: Delta(1.expr()),
            stack_pointer: Delta(0.expr()),
            // TODO: change op.
            gas_left: Delta(-OpcodeId::I32Eqz.constant_gas_cost().expr()),
            ..StepStateTransition::default()
        };
        let same_context = SameContextGadget::construct(cb, opcode, step_state_transition);

        Self {
            same_context,
            opcode_store_offset,
            store_start_block_index,
            store_start_block_inner_offset,
            store_start_block_inner_offset_helper,
            store_end_block_index,
            store_end_block_inner_offset,
            store_end_block_inner_offset_helper,
            store_value1,
            store_value2,
            mask_bits,
            offset_modulus,
            store_base,
            store_raw_value,
            store_wrapped_value,
            is_one_byte,
            is_two_bytes,
            is_four_bytes,
            is_eight_bytes,
            vtype,
            load_value1,
            load_value2,
            address_within_allocated_pages_helper,
        }
    }

    fn assign_exec_step(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        block: &Block<F>,
        _: &Transaction,
        _call: &Call,
        step: &ExecStep,
    ) -> Result<(), Error> {
        self.same_context.assign_exec_step(region, offset, step)?;

        let opcode = step.opcode.unwrap();

        cb.stack_pop(value.expr());
        cb.stack_pop(raw_address.expr());
        cb.stack_pop(pre_block_value.expr());
        cb.stack_push(update_block_value1.expr());

        let [value, raw_address, pre_block_value, update_block_value1] =
            [step.rw_indices[0], step.rw_indices[1], step.rw_indices[2], step.rw_indices[3]]
            .map(|idx| block.rws[idx].stack_value());

/*
        self.value.assign(region, offset, Value::known(value.to_scalar().unwrap()))?;
        self.value_inv.assign(region, offset, Value::known(F::from(value.as_u64()).invert().unwrap_or(F::zero())))?;
        self.res.assign(region, offset, Value::known(res.to_scalar().unwrap()))?;

        match opcode {
            OpcodeId::I64Eqz => {
                let zero_or_one = (value.as_u64() == 0) as u64;
                self.res.assign(region, offset, Value::known(F::from(zero_or_one)))?;
            }
            OpcodeId::I32Eqz => {
                let zero_or_one = (value.as_u32() == 0) as u64;
                self.res.assign(region, offset, Value::known(F::from(zero_or_one)))?;
            }
            _ => unreachable!("not supported opcode: {:?}", opcode),
        };
 
        let is_i64 = matches!(opcode,
            OpcodeId::I64Eqz
        );
        self.is_i64.assign(region, offset, Value::known(F::from(is_i64 as u64)))?;
*/

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use eth_types::{bytecode, Bytecode};
    use mock::TestContext;

    use crate::test_util::CircuitTestBuilder;

    fn run_test(bytecode: Bytecode) {
        CircuitTestBuilder::new_from_test_ctx(
            TestContext::<2, 1>::simple_ctx_with_bytecode(bytecode).unwrap(),
        ).run()
    }

/*
    #[test]
    fn test_i32_eqz() {
        run_test(bytecode! {
            I32Const[0]
            I32Eqz
            Drop
            I32Const[1]
            I32Eqz
            Drop
        });
    }

    #[test]
    fn test_i64_eqz() {
        run_test(bytecode! {
            I64Const[0]
            I64Eqz
            Drop
            I64Const[1]
            I64Eqz
            Drop
        });
    }
*/
}
