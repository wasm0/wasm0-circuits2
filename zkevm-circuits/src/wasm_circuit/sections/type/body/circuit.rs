use std::{cell::RefCell, marker::PhantomData, rc::Rc};

use halo2_proofs::{
    circuit::{Region, Value},
    plonk::{Advice, Column, ConstraintSystem, Fixed},
    poly::Rotation,
};
use itertools::Itertools;
use log::debug;

use eth_types::Field;
use gadgets::util::{and, not};

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
        sections::{
            consts::LebParams,
            r#type::{body::types::AssignType, item::circuit::WasmTypeSectionItemChip},
        },
        tables::dynamic_indexes::{circuit::DynamicIndexesChip, types::Tag},
        types::{AssignDeltaType, AssignValueType, NewWbOffsetType, SharedState},
    },
};

#[derive(Debug, Clone)]
pub struct WasmTypeSectionBodyConfig<F> {
    pub q_enable: Column<Fixed>,
    pub q_first: Column<Fixed>,
    pub q_last: Column<Fixed>,
    pub is_items_count: Column<Fixed>,
    pub is_body: Column<Fixed>,

    func_count: Column<Advice>,
    error_code: Column<Advice>,
    body_item_rev_count: Column<Advice>,

    pub section_item_chip: Rc<WasmTypeSectionItemChip<F>>,
    pub leb128_chip: Rc<LEB128Chip<F>>,
    pub dynamic_indexes_chip: Rc<DynamicIndexesChip<F>>,

    pub shared_state: Rc<RefCell<SharedState>>,

    _marker: PhantomData<F>,
}

impl<'a, F: Field> WasmTypeSectionBodyConfig<F> {}

#[derive(Debug, Clone)]
pub struct WasmTypeSectionBodyChip<F> {
    pub config: WasmTypeSectionBodyConfig<F>,
    _marker: PhantomData<F>,
}

impl<F: Field> WasmMarkupLeb128SectionAwareChip<F> for WasmTypeSectionBodyChip<F> {}

impl<F: Field> WasmCountPrefixedItemsAwareChip<F> for WasmTypeSectionBodyChip<F> {}

impl<F: Field> WasmErrorAwareChip<F> for WasmTypeSectionBodyChip<F> {
    fn error_code_col(&self) -> Column<Advice> {
        self.config.error_code
    }
}

impl<F: Field> WasmSharedStateAwareChip<F> for WasmTypeSectionBodyChip<F> {
    fn shared_state(&self) -> Rc<RefCell<SharedState>> {
        self.config.shared_state.clone()
    }
}

impl<F: Field> WasmFuncCountAwareChip<F> for WasmTypeSectionBodyChip<F> {
    fn func_count_col(&self) -> Column<Advice> {
        self.config.func_count
    }
}

impl<F: Field> WasmAssignAwareChip<F> for WasmTypeSectionBodyChip<F> {
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
            if [AssignType::IsBodyItemsCount].contains(&assign_type) {
                let p = leb_params.unwrap();
                self.config
                    .leb128_chip
                    .assign(region, assign_offset, true, p)?;
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
                AssignType::IsBodyItemsCount => {
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
                AssignType::IsBody => {
                    region
                        .assign_fixed(
                            || {
                                format!(
                                    "assign 'is_body' val {} at {}",
                                    assign_value, assign_offset
                                )
                            },
                            self.config.is_body,
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

impl<F: Field> WasmTypeSectionBodyChip<F> {
    pub fn construct(config: WasmTypeSectionBodyConfig<F>) -> Self {
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
        section_item_chip: Rc<WasmTypeSectionItemChip<F>>,
        dynamic_indexes_chip: Rc<DynamicIndexesChip<F>>,
        func_count: Column<Advice>,
        shared_state: Rc<RefCell<SharedState>>,
        body_item_rev_count: Column<Advice>,
        error_code: Column<Advice>,
    ) -> WasmTypeSectionBodyConfig<F> {
        let q_enable = cs.fixed_column();
        let q_first = cs.fixed_column();
        let q_last = cs.fixed_column();
        let is_items_count = cs.fixed_column();
        let is_body = cs.fixed_column();

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
            |vc| vc.query_fixed(section_item_chip.config.q_first, Rotation::cur()),
            |vc| vc.query_fixed(q_last, Rotation::cur()),
        );

        cs.create_gate("WasmTypeSectionBody gate", |vc| {
            let mut cb = BaseConstraintBuilder::default();

            let q_enable_expr = Self::get_selector_expr_enriched_with_error_processing(
                vc,
                q_enable,
                &shared_state.borrow(),
                error_code,
            );
            // let q_first_expr = vc.query_fixed(q_first, Rotation::cur());
            let q_last_expr = vc.query_fixed(q_last, Rotation::cur());
            let not_q_last_expr = not::expr(q_last_expr.clone());
            let is_items_count_expr = vc.query_fixed(is_items_count, Rotation::cur());
            let is_body_expr = vc.query_fixed(is_body, Rotation::cur());

            // let byte_value_expr = vc.query_advice(bytecode_table.value, Rotation::cur());

            let leb128_is_last_byte_expr =
                vc.query_fixed(leb128_chip.config.is_last_byte, Rotation::cur());

            cb.require_boolean("q_enable is boolean", q_enable_expr.clone());
            cb.require_boolean("is_items_count is boolean", is_items_count_expr.clone());
            cb.require_boolean("is_body is boolean", is_body_expr.clone());

            configure_constraints_for_q_first_and_q_last(
                &mut cb,
                vc,
                &q_enable,
                &q_first,
                &[is_items_count],
                &q_last,
                &[is_body],
            );

            cb.condition(is_items_count_expr.clone(), |cb| {
                cb.require_zero(
                    "is_items_count -> leb128",
                    not::expr(vc.query_fixed(leb128_chip.config.q_enable, Rotation::cur())),
                );
            });
            cb.require_equal(
                "is_body_expr <-> wasm_type_section_item",
                is_body_expr.clone(),
                vc.query_fixed(section_item_chip.config.q_enable, Rotation::cur()),
            );

            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_items_count+ -> is_body+",
                and::expr([not_q_last_expr.clone(), is_items_count_expr.clone()]),
                true,
                &[is_items_count, is_body],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next (last leb byte): is_items_count+ -> is_body+",
                and::expr([
                    not_q_last_expr.clone(),
                    leb128_is_last_byte_expr.clone(),
                    is_items_count_expr.clone(),
                ]),
                true,
                &[is_body],
            );
            configure_transition_check(
                &mut cb,
                vc,
                "check next: is_body+",
                and::expr([not_q_last_expr.clone(), is_body_expr.clone()]),
                true,
                &[is_body],
            );

            cb.gate(q_enable_expr.clone())
        });

        let config = WasmTypeSectionBodyConfig::<F> {
            _marker: PhantomData,

            q_enable,
            q_first,
            q_last,
            is_items_count,
            is_body,
            leb128_chip,
            section_item_chip,
            dynamic_indexes_chip,
            func_count,
            shared_state,
            body_item_rev_count,
            error_code,
        };

        config
    }

    /// updates `shared_state.dynamic_indexes_offset` to a new offset
    pub fn assign_auto(
        &self,
        region: &mut Region<F>,
        wb: &WasmBytecode,
        wb_offset: usize,
        assign_delta: AssignDeltaType,
    ) -> Result<NewWbOffsetType, Error> {
        let mut offset = wb_offset;
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
            &[AssignType::IsBodyItemsCount],
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
        offset += items_count_leb_len;

        let dynamic_indexes_offset = self.config.dynamic_indexes_chip.assign_auto(
            region,
            self.config.shared_state.borrow().dynamic_indexes_offset,
            assign_delta,
            items_count as usize,
            Tag::TypeIndex,
        )?;
        self.config.shared_state.borrow_mut().dynamic_indexes_offset = dynamic_indexes_offset;

        for _body_item_index in 0..items_count {
            body_item_rev_count -= 1;
            let item_start_offset = offset;

            let next_body_item_offset = self.config.section_item_chip.assign_auto(
                region,
                wb,
                item_start_offset,
                assign_delta,
            )?;
            for offset in item_start_offset..next_body_item_offset {
                self.assign(
                    region,
                    wb,
                    offset,
                    assign_delta,
                    &[AssignType::IsBody],
                    1,
                    None,
                )?;
            }

            for offset in item_start_offset..next_body_item_offset {
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
            offset = next_body_item_offset;
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
