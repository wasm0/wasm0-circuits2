use std::{cell::RefCell, marker::PhantomData, rc::Rc};

use halo2_proofs::{
    circuit::{Region, Value},
    plonk::{Advice, Column, ConstraintSystem, Fixed},
    poly::Rotation,
};
use log::debug;

use eth_types::Field;
use gadgets::util::{and, not, or, Expr};

use crate::{
    evm_circuit::util::constraint_builder::{BaseConstraintBuilder, ConstrainBuilderCommon},
    wasm_circuit::{
        bytecode::{bytecode::WasmBytecode, bytecode_table::WasmBytecodeTable},
        common::{
            configure_constraints_for_q_first_and_q_last, configure_transition_check,
            WasmAssignAwareChip, WasmCountPrefixedItemsAwareChip, WasmErrorAwareChip,
            WasmFuncCountAwareChip, WasmMarkupLeb128SectionAwareChip, WasmSharedStateAwareChip,
        },
        error::{remap_error_to_assign_at, Error},
        leb128::circuit::LEB128Chip,
        sections::{consts::LebParams, function::body::types::AssignType},
        types::{AssignDeltaType, AssignValueType, NewWbOffsetType, SharedState},
    },
};

#[derive(Debug, Clone)]
pub struct WasmFunctionSectionBodyConfig<F: Field> {
    pub q_enable: Column<Fixed>,
    pub q_first: Column<Fixed>,
    pub q_last: Column<Fixed>,
    pub is_items_count: Column<Fixed>,
    pub is_typeidx: Column<Fixed>,

    pub leb128_chip: Rc<LEB128Chip<F>>,

    func_count: Column<Advice>,
    body_item_rev_count: Column<Advice>,

    error_code: Column<Advice>,

    shared_state: Rc<RefCell<SharedState>>,

    _marker: PhantomData<F>,
}

impl<'a, F: Field> WasmFunctionSectionBodyConfig<F> {}

#[derive(Debug, Clone)]
pub struct WasmFunctionSectionBodyChip<F: Field> {
    pub config: WasmFunctionSectionBodyConfig<F>,
    _marker: PhantomData<F>,
}

impl<F: Field> WasmMarkupLeb128SectionAwareChip<F> for WasmFunctionSectionBodyChip<F> {}

impl<F: Field> WasmCountPrefixedItemsAwareChip<F> for WasmFunctionSectionBodyChip<F> {}

impl<F: Field> WasmErrorAwareChip<F> for WasmFunctionSectionBodyChip<F> {
    fn error_code_col(&self) -> Column<Advice> {
        self.config.error_code
    }
}

impl<F: Field> WasmSharedStateAwareChip<F> for WasmFunctionSectionBodyChip<F> {
    fn shared_state(&self) -> Rc<RefCell<SharedState>> {
        self.config.shared_state.clone()
    }
}

impl<F: Field> WasmFuncCountAwareChip<F> for WasmFunctionSectionBodyChip<F> {
    fn func_count_col(&self) -> Column<Advice> {
        self.config.func_count
    }
}

impl<F: Field> WasmAssignAwareChip<F> for WasmFunctionSectionBodyChip<F> {
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
            "assign at {} q_enable {} assign_types {:?} assign_value {} byte_val {:x?}",
            assign_offset, q_enable, assign_types, assign_value, wb.bytes[wb_offset],
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
            if [AssignType::IsItemsCount, AssignType::IsTypeidx].contains(&assign_type) {
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
                AssignType::IsTypeidx => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_typeidx' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_typeidx,
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

impl<F: Field> WasmFunctionSectionBodyChip<F> {
    pub fn construct(config: WasmFunctionSectionBodyConfig<F>) -> Self {
        let instance = Self {
            config,
            _marker: PhantomData,
        };
        instance
    }

    pub fn configure(
        cs: &mut ConstraintSystem<F>,
        _wb_table: Rc<WasmBytecodeTable>,
        leb128_chip: Rc<LEB128Chip<F>>,
        func_count: Column<Advice>,
        shared_state: Rc<RefCell<SharedState>>,
        body_item_rev_count: Column<Advice>,
        error_code: Column<Advice>,
    ) -> WasmFunctionSectionBodyConfig<F> {
        let q_enable = cs.fixed_column();
        let q_first = cs.fixed_column();
        let q_last = cs.fixed_column();
        let is_items_count = cs.fixed_column();
        let is_typeidx = cs.fixed_column();

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
            |vc| {
                let is_typeidx_expr = vc.query_fixed(is_typeidx, Rotation::cur());
                let is_first_leb_byte_expr =
                    vc.query_fixed(leb128_chip.config.is_first_byte, Rotation::cur());

                and::expr([is_typeidx_expr, is_first_leb_byte_expr])
            },
            |vc| vc.query_fixed(q_last, Rotation::cur()),
        );

        cs.create_gate("WasmFunctionSectionBody gate", |vc| {
            let mut cb = BaseConstraintBuilder::default();

            let q_enable_expr = Self::get_selector_expr_enriched_with_error_processing(
                vc,
                q_enable,
                &shared_state.borrow(),
                error_code,
            );
            let q_last_expr = vc.query_fixed(q_last, Rotation::cur());
            let not_q_last_expr = not::expr(q_last_expr.clone());
            let is_items_count_expr = vc.query_fixed(is_items_count, Rotation::cur());
            let is_typeidx_expr = vc.query_fixed(is_typeidx, Rotation::cur());

            let leb128_is_last_byte_expr =
                vc.query_fixed(leb128_chip.config.is_last_byte, Rotation::cur());

            cb.require_boolean("q_enable is boolean", q_enable_expr.clone());
            cb.require_boolean("is_items_count is boolean", is_items_count_expr.clone());
            cb.require_boolean("is_typeidx is boolean", is_typeidx_expr.clone());

            configure_constraints_for_q_first_and_q_last(
                &mut cb,
                vc,
                &q_enable,
                &q_first,
                &[is_items_count],
                &q_last,
                &[is_typeidx],
            );

            cb.require_equal(
                "exactly one mark flag active at the same time",
                is_items_count_expr.clone() + is_typeidx_expr.clone(),
                1.expr(),
            );

            cb.condition(
                or::expr([is_items_count_expr.clone(), is_typeidx_expr.clone()]),
                |cb| {
                    cb.require_equal(
                        "is_items_count || is_typeidx -> leb128",
                        vc.query_fixed(leb128_chip.config.q_enable, Rotation::cur()),
                        1.expr(),
                    )
                },
            );

            // is_items_count+ -> is_typeidx+
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_items_count+ -> is_typeidx+",
                and::expr([not_q_last_expr.clone(), is_items_count_expr.clone()]),
                true,
                &[is_items_count, is_typeidx],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_items_count+ -> is_typeidx+",
                and::expr([
                    not_q_last_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    is_items_count_expr.clone(),
                ]),
                true,
                &[is_typeidx],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_typeidx+",
                and::expr([not_q_last_expr.clone(), is_typeidx_expr.clone()]),
                true,
                &[is_typeidx],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_typeidx+",
                and::expr([
                    not_q_last_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    is_typeidx_expr.clone(),
                ]),
                true,
                &[is_typeidx],
            );

            cb.gate(q_enable_expr.clone())
        });

        let config = WasmFunctionSectionBodyConfig::<F> {
            _marker: PhantomData,

            q_enable,
            q_first,
            q_last,
            is_items_count,
            is_typeidx,
            leb128_chip,
            func_count,
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
        self.assign(
            region,
            &wb,
            offset,
            assign_delta,
            &[AssignType::QFirst],
            1,
            None,
        )?;
        offset += items_count_leb_len;

        for _item_index in 0..items_count {
            body_item_rev_count -= 1;
            let item_start_offset = offset;

            let (_typeidx_val, typeidx_val_leb_len) = self.markup_leb_section(
                region,
                wb,
                offset,
                assign_delta,
                &[AssignType::IsTypeidx],
            )?;
            offset += typeidx_val_leb_len;

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
