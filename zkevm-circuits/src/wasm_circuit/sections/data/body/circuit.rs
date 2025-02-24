use std::{cell::RefCell, marker::PhantomData, rc::Rc};

use halo2_proofs::{
    circuit::{Region, Value},
    plonk::{Advice, Column, ConstraintSystem, Fixed},
    poly::Rotation,
};
use itertools::Itertools;
use log::debug;

use eth_types::Field;
use gadgets::{
    binary_number::BinaryNumberChip,
    util::{and, not, or, Expr},
};

use crate::{
    evm_circuit::util::constraint_builder::{BaseConstraintBuilder, ConstrainBuilderCommon},
    wasm_circuit::{
        bytecode::{bytecode::WasmBytecode, bytecode_table::WasmBytecodeTable},
        common::{
            configure_constraints_for_q_first_and_q_last, configure_transition_check,
            WasmAssignAwareChip, WasmCountPrefixedItemsAwareChip, WasmErrorAwareChip,
            WasmFuncCountAwareChip, WasmLenPrefixedBytesSpanAwareChip,
            WasmMarkupLeb128SectionAwareChip, WasmSharedStateAwareChip,
        },
        consts::WASM_BLOCK_END,
        error::{
            remap_error, remap_error_to_assign_at, remap_error_to_invalid_enum_value_at, Error,
        },
        leb128::circuit::LEB128Chip,
        sections::{consts::LebParams, data::body::types::AssignType},
        tables::dynamic_indexes::{
            circuit::DynamicIndexesChip,
            types::{LookupArgsParams, Tag},
        },
        types::{
            AssignDeltaType, AssignValueType, MemSegmentType, NewWbOffsetType, NumericInstruction,
            SharedState,
        },
    },
};

#[derive(Debug, Clone)]
pub struct WasmDataSectionBodyConfig<F: Field> {
    pub q_enable: Column<Fixed>,
    pub q_first: Column<Fixed>,
    pub q_last: Column<Fixed>,
    pub is_items_count: Column<Fixed>,
    pub is_memidx: Column<Fixed>,
    pub is_mem_segment_type: Column<Fixed>,
    pub is_mem_segment_size_opcode: Column<Fixed>,
    pub is_mem_segment_size: Column<Fixed>,
    pub is_block_end: Column<Fixed>,
    pub is_mem_segment_len: Column<Fixed>,
    pub is_mem_segment_bytes: Column<Fixed>,

    pub is_mem_segment_type_ctx: Column<Fixed>,

    pub leb128_chip: Rc<LEB128Chip<F>>,
    pub dynamic_indexes_chip: Rc<DynamicIndexesChip<F>>,
    pub mem_segment_type: Column<Advice>,
    pub mem_segment_type_chip: Rc<BinaryNumberChip<F, MemSegmentType, 8>>,

    func_count: Column<Advice>,
    body_byte_rev_index: Column<Advice>,
    body_item_rev_count: Column<Advice>,

    error_code: Column<Advice>,

    shared_state: Rc<RefCell<SharedState>>,

    _marker: PhantomData<F>,
}

impl<'a, F: Field> WasmDataSectionBodyConfig<F> {}

#[derive(Debug, Clone)]
pub struct WasmDataSectionBodyChip<F: Field> {
    pub config: WasmDataSectionBodyConfig<F>,
    _marker: PhantomData<F>,
}

impl<F: Field> WasmMarkupLeb128SectionAwareChip<F> for WasmDataSectionBodyChip<F> {}

impl<F: Field> WasmCountPrefixedItemsAwareChip<F> for WasmDataSectionBodyChip<F> {}

impl<F: Field> WasmLenPrefixedBytesSpanAwareChip<F> for WasmDataSectionBodyChip<F> {}

impl<F: Field> WasmErrorAwareChip<F> for WasmDataSectionBodyChip<F> {
    fn error_code_col(&self) -> Column<Advice> {
        self.config.error_code
    }
}

impl<F: Field> WasmSharedStateAwareChip<F> for WasmDataSectionBodyChip<F> {
    fn shared_state(&self) -> Rc<RefCell<SharedState>> {
        self.config.shared_state.clone()
    }
}

impl<F: Field> WasmFuncCountAwareChip<F> for WasmDataSectionBodyChip<F> {
    fn func_count_col(&self) -> Column<Advice> {
        self.config.func_count
    }
}

impl<F: Field> WasmAssignAwareChip<F> for WasmDataSectionBodyChip<F> {
    type AssignType = AssignType;

    fn assign_internal(
        &self,
        region: &mut Region<F>,
        wb: &WasmBytecode,
        wb_offset: usize,
        assign_delta: AssignDeltaType,
        assign_types: &[Self::AssignType],
        assign_value: AssignValueType,
        leb_params: Option<LebParams>,
    ) -> Result<(), Error> {
        let q_enable = true;
        let assign_offset = wb_offset + assign_delta;
        debug!(
            "assign at {} q_enable {} assign_types {:?} assign_value {} byte_val {:x?} leb_params {:?}",
            assign_offset,
            q_enable,
            assign_types,
            assign_value,
            wb.bytes[wb_offset],
            leb_params,
        );
        region
            .assign_fixed(
                || format!("assign 'q_enable' val {} at {}", q_enable, assign_offset),
                self.config.q_enable,
                assign_offset,
                || Value::known(F::from(q_enable as u64)),
            )
            .map_err(remap_error_to_assign_at(assign_offset))?;
        self.assign_func_count(region, assign_offset)?;

        for assign_type in assign_types {
            if [
                AssignType::IsItemsCount,
                AssignType::IsMemSegmentSize,
                AssignType::IsMemSegmentLen,
            ]
            .contains(&assign_type)
            {
                let p = leb_params.unwrap();
                self.config
                    .leb128_chip
                    .assign(region, assign_offset, q_enable, p)?;
            }
            match assign_type {
                AssignType::QFirst => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'q_first' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.q_first,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::QLast => {
                    region
                        .assign_fixed(
                            || format!("assign 'q_last' val {} at {}", assign_value, assign_offset),
                            self.config.q_last,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::IsItemsCount => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_items_count' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_items_count,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::IsMemSegmentType => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_mem_segment_type' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_mem_segment_type,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::IsMemSegmentSizeOpcode => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_mem_segment_size_opcode' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_mem_segment_size_opcode,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::IsMemSegmentSize => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_mem_segment_size' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_mem_segment_size,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::IsBlockEnd => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_block_end' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_block_end,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::IsMemSegmentLen => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_mem_segment_len' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_mem_segment_len,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::IsMemSegmentBytes => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_mem_segment_bytes' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_mem_segment_bytes,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::IsMemSegmentTypeCtx => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_mem_segment_type_ctx' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_mem_segment_type_ctx,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::MemSegmentType => {
                    region
                        .assign_advice(
                            || {
                                format!(
                                    "assign 'mem_segment_type' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.mem_segment_type,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                    let mem_segment_type: MemSegmentType = (assign_value as u8)
                        .try_into()
                        .map_err(remap_error_to_invalid_enum_value_at(assign_offset))?;
                    self.config
                        .mem_segment_type_chip
                        .assign(region, assign_offset, &mem_segment_type)
                        .map_err(remap_error(Error::FatalAssignExternalChip))?;
                }
                AssignType::IsMemIndex => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_mem_index' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_memidx,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::BodyByteRevIndex => {
                    region
                        .assign_advice(
                            || {
                                format!(
                                    "assign 'body_byte_rev_index' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.body_byte_rev_index,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::BodyItemRevCount => {
                    region
                        .assign_advice(
                            || {
                                format!(
                                    "assign 'body_item_rev_count' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.body_item_rev_count,
                            assign_offset,
                            || Value::known(F::from(assign_value)),
                        )
                        .map_err(remap_error_to_assign_at(assign_offset))?;
                }
                AssignType::ErrorCode => {
                    self.assign_error_code(region, assign_offset, None)?;
                }
            }
        }
        Ok(())
    }
}

impl<F: Field> WasmDataSectionBodyChip<F> {
    pub fn construct(config: WasmDataSectionBodyConfig<F>) -> Self {
        let instance = Self {
            config,
            _marker: PhantomData,
        };
        instance
    }

    pub fn configure(
        cs: &mut ConstraintSystem<F>,
        wb_table: Rc<WasmBytecodeTable>,
        leb128_chip: Rc<LEB128Chip<F>>,
        dynamic_indexes_chip: Rc<DynamicIndexesChip<F>>,
        func_count: Column<Advice>,
        shared_state: Rc<RefCell<SharedState>>,
        body_byte_rev_index: Column<Advice>,
        body_item_rev_count: Column<Advice>,
        error_code: Column<Advice>,
        bytecode_number: Column<Advice>,
    ) -> WasmDataSectionBodyConfig<F> {
        let q_enable = cs.fixed_column();
        let q_first = cs.fixed_column();
        let q_last = cs.fixed_column();
        let is_items_count = cs.fixed_column();
        let is_memidx = cs.fixed_column();
        let is_mem_segment_type = cs.fixed_column();
        let is_mem_segment_size_opcode = cs.fixed_column();
        let is_mem_segment_size = cs.fixed_column();
        let is_block_end = cs.fixed_column();
        let is_mem_segment_len = cs.fixed_column();
        let is_mem_segment_bytes = cs.fixed_column();

        let is_mem_segment_type_ctx = cs.fixed_column();
        let mem_segment_type = cs.advice_column();

        let config =
            BinaryNumberChip::configure(cs, is_mem_segment_type_ctx, Some(mem_segment_type.into()));
        let mem_segment_type_chip = Rc::new(BinaryNumberChip::construct(config));

        dynamic_indexes_chip.lookup_args(
            "data section has valid setup for data indexes",
            cs,
            |vc| {
                let cond = vc.query_fixed(is_items_count, Rotation::cur());
                let cond = cond
                    * Self::get_selector_expr_enriched_with_error_processing(
                        vc,
                        q_enable,
                        &shared_state.borrow(),
                        error_code,
                    );
                LookupArgsParams {
                    cond,
                    bytecode_number: vc.query_advice(bytecode_number, Rotation::cur()),
                    index: vc.query_advice(leb128_chip.config.sn, Rotation::cur()),
                    tag: Tag::DataIndex.expr(),
                    is_terminator: true.expr(),
                }
            },
        );

        Self::configure_len_prefixed_bytes_span_checks(
            cs,
            leb128_chip.as_ref(),
            |vc| vc.query_fixed(is_mem_segment_bytes, Rotation::cur()),
            body_byte_rev_index,
            |vc| {
                let not_q_last_expr = not::expr(vc.query_fixed(q_last, Rotation::cur()));
                let is_mem_segment_len_expr = vc.query_fixed(is_mem_segment_len, Rotation::cur());
                let is_mem_segment_type_next_expr =
                    vc.query_fixed(is_mem_segment_type, Rotation::next());

                and::expr([
                    not_q_last_expr,
                    is_mem_segment_len_expr,
                    is_mem_segment_type_next_expr,
                ])
            },
            |vc| {
                let q_last_expr = vc.query_fixed(q_last, Rotation::cur());
                let is_block_end_expr = vc.query_fixed(is_mem_segment_bytes, Rotation::cur());
                let is_mem_segment_type_next_expr =
                    vc.query_fixed(is_mem_segment_type, Rotation::next());

                or::expr([
                    q_last_expr,
                    and::expr([is_block_end_expr, is_mem_segment_type_next_expr]),
                ])
            },
        );

        Self::configure_count_prefixed_items_checks(
            cs,
            leb128_chip.as_ref(),
            body_item_rev_count,
            |vc| vc.query_fixed(is_items_count, Rotation::cur()),
            |vc| {
                let q_enable_expr = Self::get_selector_expr_enriched_with_error_processing(
                    vc,
                    q_enable,
                    &shared_state.borrow(),
                    error_code,
                );
                let is_items_count_expr = vc.query_fixed(is_items_count, Rotation::cur());

                and::expr([q_enable_expr, not::expr(is_items_count_expr)])
            },
            |vc| vc.query_fixed(is_mem_segment_type, Rotation::cur()),
            |vc| vc.query_fixed(q_last, Rotation::cur()),
        );

        cs.create_gate("WasmDataSectionBody gate", |vc| {
            let mut cb = BaseConstraintBuilder::default();

            let q_enable_expr = Self::get_selector_expr_enriched_with_error_processing(vc, q_enable, &shared_state.borrow(), error_code);
            let q_last_expr = vc.query_fixed(q_last, Rotation::cur());
            let not_q_last_expr = not::expr(q_last_expr.clone());
            let is_items_count_expr = vc.query_fixed(is_items_count, Rotation::cur());
            let is_mem_segment_type_expr = vc.query_fixed(is_mem_segment_type, Rotation::cur());
            let is_mem_index_expr = vc.query_fixed(is_memidx, Rotation::cur());
            let is_mem_segment_size_opcode_expr = vc.query_fixed(is_mem_segment_size_opcode, Rotation::cur());
            let is_mem_segment_size_expr = vc.query_fixed(is_mem_segment_size, Rotation::cur());
            let is_block_end_expr = vc.query_fixed(is_block_end, Rotation::cur());
            let is_mem_segment_len_expr = vc.query_fixed(is_mem_segment_len, Rotation::cur());
            let is_mem_segment_bytes_expr = vc.query_fixed(is_mem_segment_bytes, Rotation::cur());

            let is_mem_segment_type_ctx_prev_expr = vc.query_fixed(is_mem_segment_type_ctx, Rotation::prev());
            let is_mem_segment_type_ctx_expr = vc.query_fixed(is_mem_segment_type_ctx, Rotation::cur());

            let byte_val_expr = vc.query_advice(wb_table.value, Rotation::cur());
            let mem_segment_type_expr = vc.query_advice(mem_segment_type, Rotation::cur());

            let leb128_is_last_byte_expr = vc.query_fixed(leb128_chip.config.is_last_byte, Rotation::cur());

            cb.require_boolean("q_enable is boolean", q_enable_expr.clone());
            cb.require_boolean("is_items_count is boolean", is_items_count_expr.clone());
            cb.require_boolean("is_mem_segment_type is boolean", is_mem_segment_type_expr.clone());
            cb.require_boolean("is_mem_index is boolean", is_mem_index_expr.clone());
            cb.require_boolean("is_mem_segment_size_opcode is boolean", is_mem_segment_size_opcode_expr.clone());
            cb.require_boolean("is_mem_segment_size is boolean", is_mem_segment_size_expr.clone());
            cb.require_boolean("is_block_end is boolean", is_block_end_expr.clone());
            cb.require_boolean("is_mem_segment_len is boolean", is_mem_segment_len_expr.clone());
            cb.require_boolean("is_mem_segment_bytes is boolean", is_mem_segment_bytes_expr.clone());

            cb.require_equal(
                "exactly one mark flag active at the same time",
                is_items_count_expr.clone()
                    + is_mem_segment_type_expr.clone()
                    + is_mem_index_expr.clone()
                    + is_mem_segment_size_opcode_expr.clone()
                    + is_mem_segment_size_expr.clone()
                    + is_block_end_expr.clone()
                    + is_mem_segment_len_expr.clone()
                    + is_mem_segment_bytes_expr.clone()
                ,
                1.expr(),
            );

            cb.condition(
                or::expr([
                    is_items_count_expr.clone(),
                    is_mem_index_expr.clone(),
                    is_mem_segment_size_expr.clone(),
                    is_mem_segment_len_expr.clone(),
                ]),
                |cb| {
                    cb.require_equal(
                        "is_items_count || is_mem_index || is_mem_segment_size || is_mem_segment_len -> leb128",
                        vc.query_fixed(leb128_chip.config.q_enable, Rotation::cur()),
                        1.expr(),
                    )
                }
            );

            configure_constraints_for_q_first_and_q_last(
                &mut cb,
                vc,
                &q_enable,
                &q_first,
                &[is_items_count],
                &q_last,
                &[is_mem_segment_len, is_mem_segment_bytes],
            );

            // constraints for is_mem_segment_type_ctx
            cb.condition(
                is_mem_segment_type_ctx_expr.clone(),
                |cb| {
                    cb.require_equal(
                        "is_mem_segment_type_ctx => specific flags are active",
                        is_mem_segment_type_expr.clone()
                            + is_mem_index_expr.clone()
                            + is_mem_segment_size_opcode_expr.clone()
                            + is_mem_segment_size_expr.clone()
                            + is_block_end_expr.clone()
                            + is_mem_segment_len_expr.clone()
                            + is_mem_segment_bytes_expr.clone(),
                        1.expr(),
                    )
                }
            );
            // constraints for AssignType::MemSegmentType
            cb.condition(
                is_mem_segment_type_expr.clone(),
                |cb| {
                    cb.require_equal(
                        "is_mem_segment_type => mem_segment_type=byte_val",
                        mem_segment_type_expr.clone(),
                        byte_val_expr.clone(),
                    );
                }
            );
            cb.condition(
                and::expr([
                    is_mem_segment_type_ctx_expr.clone(),
                    is_mem_segment_type_ctx_prev_expr.clone(),
                ]),
                |cb| {
                    let mem_segment_type_prev_expr = vc.query_advice(mem_segment_type, Rotation::prev());
                    cb.require_equal(
                        "is_mem_segment_type_ctx && prev.is_mem_segment_type_ctx => mem_segment_type=prev.mem_segment_type",
                        mem_segment_type_prev_expr.clone(),
                        mem_segment_type_expr.clone(),
                    );
                }
            );

            let mem_segment_type_is_active_expr = mem_segment_type_chip.config.value_equals(MemSegmentType::Active, Rotation::cur())(vc);
            let mem_segment_type_is_passive_expr = mem_segment_type_chip.config.value_equals(MemSegmentType::Passive, Rotation::cur())(vc);
            let mem_segment_type_is_active_variadic_expr = mem_segment_type_chip.config.value_equals(MemSegmentType::ActiveVariadic, Rotation::cur())(vc);
            // constraints for is_mem_segment_type{1}=MemSegmentType::Active:
            // is_items_count+ -> item+ (is_mem_segment_type{1} -> is_mem_segment_size_opcode{1} -> is_mem_segment_size+ -> is_block_end{1} -> is_mem_segment_len+ -> is_mem_segment_bytes*)
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_items_count+ -> item+ (is_mem_segment_type{1} ...",
                and::expr([
                    not_q_last_expr.clone(),
                    is_items_count_expr.clone(),
                    mem_segment_type_is_active_expr.clone(),
                ]),
                true,
                &[is_items_count, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_items_count+ -> item+ (is_mem_segment_type{1} ...",
                and::expr([
                    not_q_last_expr.clone(),
                    is_items_count_expr.clone(),
                    mem_segment_type_is_active_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                ]),
                true,
                &[is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_type{1} -> is_mem_segment_size_opcode{1}",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_type_expr.clone(),
                    mem_segment_type_is_active_expr.clone(),
                ]),
                true,
                &[is_mem_segment_size_opcode],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_size_opcode{1} -> is_mem_segment_size+",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_size_opcode_expr.clone(),
                    mem_segment_type_is_active_expr.clone(),
                ]),
                true,
                &[is_mem_segment_size],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_size+ -> is_block_end{1}",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_size_expr.clone(),
                    mem_segment_type_is_active_expr.clone(),
                ]),
                true,
                &[is_mem_segment_size, is_block_end],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_mem_segment_size+ -> is_block_end{1}",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_size_expr.clone(),
                    mem_segment_type_is_active_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                ]),
                true,
                &[is_block_end],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_block_end{1} -> is_mem_segment_len+",
                and::expr([
                    not_q_last_expr.clone(),
                    is_block_end_expr.clone(),
                    mem_segment_type_is_active_expr.clone(),
                ]),
                true,
                &[is_mem_segment_len],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_len+ -> is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_len_expr.clone(),
                ]),
                true,
                &[is_mem_segment_len, is_mem_segment_bytes, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_mem_segment_len+ -> is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_len_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                ]),
                true,
                &[is_mem_segment_bytes, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_bytes_expr.clone(),
                ]),
                true,
                &[is_mem_segment_bytes, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_bytes_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                ]),
                true,
                &[is_mem_segment_type],
            );
            // constraints for is_mem_segment_type{1}=MemSegmentType::Passive:
            // is_items_count+ -> item+ (is_mem_segment_len{1} -> is_mem_segment_bytes*
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_items_count+ -> item+ (is_mem_segment_len{1} ...",
                and::expr([
                    not_q_last_expr.clone(),
                    is_items_count_expr.clone(),
                    mem_segment_type_is_passive_expr.clone(),
                ]),
                true,
                &[is_items_count, is_mem_segment_len],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_items_count+ -> item+ (is_mem_segment_len{1} ...",
                and::expr([
                    not_q_last_expr.clone(),
                    is_items_count_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    mem_segment_type_is_passive_expr.clone(),
                ]),
                true,
                &[is_mem_segment_len],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_len+ -> is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_len_expr.clone(),
                ]),
                true,
                &[is_mem_segment_len, is_mem_segment_bytes, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_mem_segment_len+ -> is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    is_mem_segment_len_expr.clone(),
                ]),
                true,
                &[is_mem_segment_bytes, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_bytes_expr.clone(),
                ]),
                true,
                &[is_mem_segment_bytes, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    is_mem_segment_bytes_expr.clone(),
                ]),
                true,
                &[is_mem_segment_type],
            );
            // constraints for is_mem_segment_type{1}=MemSegmentType::ActiveVariadic:
            //  is_items_count+ -> item+ (is_mem_segment_type{1} -> is_mem_index+ -> is_mem_segment_size_opcode{1} -> is_mem_segment_size+ -> is_block_end{1} -> is_mem_segment_len+ -> is_mem_segment_bytes*)
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_items_count+ -> item+ (is_mem_segment_type{1} ...",
                and::expr([
                    not_q_last_expr.clone(),
                    is_items_count_expr.clone(),
                    mem_segment_type_is_active_variadic_expr.clone(),
                ]),
                true,
                &[is_items_count, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_items_count+ -> item+ (is_mem_segment_type{1} ...",
                and::expr([
                    not_q_last_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    is_items_count_expr.clone(),
                    mem_segment_type_is_active_variadic_expr.clone(),
                ]),
                true,
                &[is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_type{1} -> is_mem_index+",
                and::expr([
                    is_mem_segment_type_expr.clone(),
                    mem_segment_type_is_active_variadic_expr.clone(),
                    not_q_last_expr.clone(),
                ]),
                true,
                &[is_memidx],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_index+ -> is_mem_segment_size_opcode{1}",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_index_expr.clone(),
                    mem_segment_type_is_active_variadic_expr.clone(),
                ]),
                true,
                &[is_memidx, is_mem_segment_size_opcode],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_mem_index+ -> is_mem_segment_size_opcode{1}",
                and::expr([
                    not_q_last_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    is_mem_index_expr.clone(),
                    mem_segment_type_is_active_variadic_expr.clone(),
                ]),
                true,
                &[is_mem_segment_size_opcode],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_size_opcode{1} -> is_mem_segment_size+",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_size_opcode_expr.clone(),
                    mem_segment_type_is_active_variadic_expr.clone(),
                ]),
                true,
                &[is_mem_segment_size],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_size+ -> is_block_end{1}",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_size_expr.clone(),
                    mem_segment_type_is_active_variadic_expr.clone(),
                ]),
                true,
                &[is_mem_segment_size, is_block_end],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_mem_segment_size+ -> is_block_end{1}",
                and::expr([
                    not_q_last_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    is_mem_segment_size_expr.clone(),
                    mem_segment_type_is_active_variadic_expr.clone(),
                ]),
                true,
                &[is_block_end],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_block_end{1} -> is_mem_segment_len+",
                and::expr([
                    not_q_last_expr.clone(),
                    is_block_end_expr.clone(),
                    mem_segment_type_is_active_variadic_expr.clone(),
                ]),
                true,
                &[is_mem_segment_len],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_len+ -> is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_len_expr.clone(),
                ]),
                true,
                &[is_mem_segment_len, is_mem_segment_bytes, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_mem_segment_len+ -> is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    is_mem_segment_len_expr.clone(),
                ]),
                true,
                &[is_mem_segment_bytes, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    is_mem_segment_bytes_expr.clone(),
                ]),
                true,
                &[is_mem_segment_bytes, is_mem_segment_type],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_mem_segment_bytes*",
                and::expr([
                    not_q_last_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    is_mem_segment_bytes_expr.clone(),
                ]),
                true,
                &[is_mem_segment_type],
            );

            cb.condition(
                is_block_end_expr.clone(),
                |cb| {
                    cb.require_equal(
                        "is_block_end -> byte value = WASM_BLOCK_END",
                        byte_val_expr.clone(),
                        WASM_BLOCK_END.expr(),
                    )
                }
            );

            cb.condition(
                is_mem_segment_type_expr.clone(),
                |cb| {
                    cb.require_in_set(
                        "is_mem_segment_type -> byte value is valid",
                        byte_val_expr.clone(),
                        vec![
                            MemSegmentType::Active.expr(),
                            MemSegmentType::Passive.expr(),
                            MemSegmentType::ActiveVariadic.expr(),
                        ],
                    )
                }
            );

            cb.condition(
                is_mem_segment_size_opcode_expr.clone(),
                |cb| {
                    cb.require_in_set(
                        "is_mem_segment_size_opcode -> byte value is valid",
                        byte_val_expr.clone(),
                        vec![
                            NumericInstruction::I32Const.expr(),
                        ],
                    )
                }
            );

            cb.gate(q_enable_expr.clone())
        });

        let config = WasmDataSectionBodyConfig::<F> {
            _marker: PhantomData,

            q_enable,
            q_first,
            q_last,
            is_items_count,
            is_memidx,
            is_mem_segment_type,
            is_mem_segment_size_opcode,
            is_mem_segment_size,
            is_block_end,
            is_mem_segment_len,
            is_mem_segment_bytes,
            is_mem_segment_type_ctx,
            leb128_chip,
            dynamic_indexes_chip,
            mem_segment_type,
            mem_segment_type_chip,
            func_count,
            body_byte_rev_index,
            body_item_rev_count,
            error_code,
            shared_state,
        };

        config
    }

    pub fn assign_auto(
        &self,
        region: &mut Region<F>,
        wb: &WasmBytecode,
        wb_offset: usize,
        assign_delta: AssignDeltaType,
    ) -> Result<NewWbOffsetType, Error> {
        let mut offset = wb_offset;

        // items_count+
        self.assign(
            region,
            &wb,
            offset,
            assign_delta,
            &[AssignType::QFirst],
            1,
            None,
        )?;
        let (items_count, items_count_leb_len) = self.markup_leb_section(
            region,
            wb,
            offset,
            assign_delta,
            &[AssignType::IsItemsCount],
        )?;
        let mut body_item_rev_count = items_count;
        for offset in offset..offset + items_count_leb_len {
            self.assign(
                region,
                &wb,
                offset,
                assign_delta,
                &[AssignType::BodyItemRevCount],
                body_item_rev_count,
                None,
            )?;
        }
        let dynamic_indexes_offset = self.config.dynamic_indexes_chip.assign_auto(
            region,
            self.config.shared_state.borrow().dynamic_indexes_offset,
            assign_delta,
            items_count as usize,
            Tag::DataIndex,
        )?;
        self.config.shared_state.borrow_mut().dynamic_indexes_offset = dynamic_indexes_offset;
        offset += items_count_leb_len;

        for _item_index in 0..items_count {
            body_item_rev_count -= 1;
            let item_start_offset = offset;

            // is_mem_segment_type{1}
            let mem_segment_type_val = wb.bytes[offset];
            let mem_segment_type: MemSegmentType = mem_segment_type_val
                .try_into()
                .map_err(remap_error_to_invalid_enum_value_at(offset))?;
            self.assign(
                region,
                wb,
                offset,
                assign_delta,
                &[
                    AssignType::IsMemSegmentType,
                    AssignType::IsMemSegmentTypeCtx,
                ],
                1,
                None,
            )?;
            self.assign(
                region,
                wb,
                offset,
                assign_delta,
                &[AssignType::MemSegmentType],
                mem_segment_type_val as u64,
                None,
            )?;
            offset += 1;

            match mem_segment_type {
                MemSegmentType::Active => {
                    // is_mem_segment_size_opcode{1}
                    self.assign(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[
                            AssignType::IsMemSegmentSizeOpcode,
                            AssignType::IsMemSegmentTypeCtx,
                        ],
                        1,
                        None,
                    )?;
                    self.assign(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[AssignType::MemSegmentType],
                        mem_segment_type_val as u64,
                        None,
                    )?;
                    offset += 1;

                    // is_mem_segment_size+
                    let (_mem_segment_size, mem_segment_size_leb_len) = self.markup_leb_section(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[
                            AssignType::IsMemSegmentSize,
                            AssignType::IsMemSegmentTypeCtx,
                        ],
                    )?;
                    for offset in offset..offset + mem_segment_size_leb_len {
                        self.assign(
                            region,
                            wb,
                            offset,
                            assign_delta,
                            &[AssignType::MemSegmentType],
                            mem_segment_type_val as u64,
                            None,
                        )?;
                    }
                    offset += mem_segment_size_leb_len;

                    // is_block_end{1}
                    self.assign(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[AssignType::IsBlockEnd, AssignType::IsMemSegmentTypeCtx],
                        1,
                        None,
                    )?;
                    self.assign(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[AssignType::MemSegmentType],
                        mem_segment_type_val as u64,
                        None,
                    )?;
                    offset += 1;

                    // is_mem_segment_len+
                    let (mem_segment_len, mem_segment_len_leb_len) = self.markup_leb_section(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[AssignType::IsMemSegmentLen, AssignType::IsMemSegmentTypeCtx],
                    )?;
                    let mem_segment_len_last_byte_offset = offset + mem_segment_len_leb_len - 1;
                    let mem_segment_last_byte_offset =
                        mem_segment_len_last_byte_offset + mem_segment_len as usize;
                    for offset in mem_segment_len_last_byte_offset..=mem_segment_last_byte_offset {
                        self.assign(
                            region,
                            &wb,
                            offset,
                            assign_delta,
                            &[AssignType::BodyByteRevIndex],
                            (mem_segment_last_byte_offset - offset) as u64,
                            None,
                        )?;
                    }
                    for offset in offset..offset + mem_segment_len_leb_len {
                        self.assign(
                            region,
                            wb,
                            offset,
                            assign_delta,
                            &[AssignType::MemSegmentType],
                            mem_segment_type_val as u64,
                            None,
                        )?;
                    }
                    offset += mem_segment_len_leb_len;

                    // is_mem_segment_bytes*
                    for rel_offset in 0..(mem_segment_len as usize) {
                        self.assign(
                            region,
                            wb,
                            offset + rel_offset,
                            assign_delta,
                            &[
                                AssignType::IsMemSegmentBytes,
                                AssignType::IsMemSegmentTypeCtx,
                            ],
                            1,
                            None,
                        )?;
                        for offset in offset..offset + mem_segment_len_leb_len {
                            self.assign(
                                region,
                                wb,
                                offset,
                                assign_delta,
                                &[AssignType::MemSegmentType],
                                mem_segment_type_val as u64,
                                None,
                            )?;
                        }
                    }
                    offset += mem_segment_len as usize;
                }
                MemSegmentType::Passive => {
                    // is_mem_segment_len+
                    let (mem_segment_len, mem_segment_len_leb_len) = self.markup_leb_section(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[AssignType::IsMemSegmentLen, AssignType::IsMemSegmentTypeCtx],
                    )?;
                    let mem_segment_len_last_byte_offset = offset + mem_segment_len_leb_len - 1;
                    let mem_segment_last_byte_offset =
                        mem_segment_len_last_byte_offset + mem_segment_len as usize;
                    for offset in mem_segment_len_last_byte_offset..=mem_segment_last_byte_offset {
                        self.assign(
                            region,
                            &wb,
                            offset,
                            assign_delta,
                            &[AssignType::BodyByteRevIndex],
                            (mem_segment_last_byte_offset - offset) as u64,
                            None,
                        )?;
                    }
                    for offset in offset..offset + mem_segment_len_leb_len {
                        self.assign(
                            region,
                            wb,
                            offset,
                            assign_delta,
                            &[AssignType::MemSegmentType],
                            mem_segment_type_val as u64,
                            None,
                        )?;
                    }
                    offset += mem_segment_len_leb_len;

                    // is_mem_segment_bytes*
                    for rel_offset in 0..(mem_segment_len as usize) {
                        self.assign(
                            region,
                            wb,
                            offset + rel_offset,
                            assign_delta,
                            &[
                                AssignType::IsMemSegmentBytes,
                                AssignType::IsMemSegmentTypeCtx,
                            ],
                            1,
                            None,
                        )?;
                        self.assign(
                            region,
                            wb,
                            offset,
                            assign_delta,
                            &[AssignType::MemSegmentType],
                            mem_segment_type_val as u64,
                            None,
                        )?;
                    }
                    offset += mem_segment_len as usize;
                }
                MemSegmentType::ActiveVariadic => {
                    // is_mem_index+
                    let (_mem_index, mem_index_leb_len) = self.markup_leb_section(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[AssignType::IsMemIndex, AssignType::IsMemSegmentTypeCtx],
                    )?;
                    for offset in offset..offset + mem_index_leb_len {
                        self.assign(
                            region,
                            wb,
                            offset,
                            assign_delta,
                            &[AssignType::MemSegmentType],
                            mem_segment_type_val as u64,
                            None,
                        )?;
                    }
                    offset += mem_index_leb_len;

                    // is_mem_segment_size_opcode{1}
                    self.assign(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[
                            AssignType::IsMemSegmentSizeOpcode,
                            AssignType::IsMemSegmentTypeCtx,
                        ],
                        1,
                        None,
                    )?;
                    self.assign(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[AssignType::MemSegmentType],
                        mem_segment_type_val as u64,
                        None,
                    )?;
                    offset += 1;

                    // is_mem_segment_size+
                    let (_mem_segment_size, mem_segment_size_leb_len) = self.markup_leb_section(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[
                            AssignType::IsMemSegmentSize,
                            AssignType::IsMemSegmentTypeCtx,
                        ],
                    )?;
                    for offset in offset..offset + mem_segment_size_leb_len {
                        self.assign(
                            region,
                            wb,
                            offset,
                            assign_delta,
                            &[AssignType::MemSegmentType],
                            mem_segment_type_val as u64,
                            None,
                        )?;
                    }
                    offset += mem_segment_size_leb_len;

                    // is_block_end{1}
                    self.assign(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[AssignType::IsBlockEnd, AssignType::IsMemSegmentTypeCtx],
                        1,
                        None,
                    )?;
                    self.assign(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[AssignType::MemSegmentType],
                        mem_segment_type_val as u64,
                        None,
                    )?;
                    offset += 1;

                    // is_mem_segment_len+
                    let (mem_segment_len, mem_segment_len_leb_len) = self.markup_leb_section(
                        region,
                        wb,
                        offset,
                        assign_delta,
                        &[AssignType::IsMemSegmentLen, AssignType::IsMemSegmentTypeCtx],
                    )?;
                    for offset in offset..offset + mem_segment_len_leb_len {
                        self.assign(
                            region,
                            wb,
                            offset,
                            assign_delta,
                            &[AssignType::MemSegmentType],
                            mem_segment_type_val as u64,
                            None,
                        )?;
                    }
                    offset += mem_segment_len_leb_len;

                    // is_mem_segment_bytes*
                    for rel_offset in 0..(mem_segment_len as usize) {
                        self.assign(
                            region,
                            wb,
                            offset + rel_offset,
                            assign_delta,
                            &[
                                AssignType::IsMemSegmentBytes,
                                AssignType::IsMemSegmentTypeCtx,
                            ],
                            1,
                            None,
                        )?;
                        for offset in offset..offset + mem_segment_len_leb_len {
                            self.assign(
                                region,
                                wb,
                                offset,
                                assign_delta,
                                &[AssignType::MemSegmentType],
                                mem_segment_type_val as u64,
                                None,
                            )?;
                        }
                    }
                    offset += mem_segment_len as usize;
                }
            }

            for offset in item_start_offset..offset {
                self.assign(
                    region,
                    &wb,
                    offset,
                    assign_delta,
                    &[AssignType::BodyItemRevCount],
                    body_item_rev_count,
                    None,
                )?;
            }
        }

        if offset != wb_offset {
            self.assign(
                region,
                &wb,
                offset - 1,
                assign_delta,
                &[AssignType::QLast],
                1,
                None,
            )?;
        }

        Ok(offset)
    }
}
