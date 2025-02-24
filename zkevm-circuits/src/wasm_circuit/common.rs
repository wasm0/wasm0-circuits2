use std::{cell::RefCell, rc::Rc};

use halo2_proofs::{
    circuit::{Chip, Region, Value},
    plonk::{Advice, Column, ConstraintSystem, Expression, Fixed, VirtualCells},
    poly::Rotation,
};
use log::debug;
use num_traits::checked_pow;
use wabt::wat2wasm;
use wasmbin::{
    io::{DecodeError, Encode},
    sections::Kind,
    visit::{Visit, VisitError},
    Module,
};

use eth_types::Field;
use gadgets::{
    binary_number::BinaryNumberChip,
    less_than::LtChip,
    util::{and, not, or, Expr},
};

use crate::{
    evm_circuit::util::constraint_builder::{BaseConstraintBuilder, ConstrainBuilderCommon},
    wasm_circuit::{
        bytecode::{bytecode::WasmBytecode, bytecode_table::WasmBytecodeTable},
        consts::MAX_LEB128_BYTES,
        error::{
            error_index_out_of_bounds, remap_error_to_assign_at, remap_error_to_compute_value_at,
            validate_wb_offset, Error,
        },
        leb128::{
            circuit::LEB128Chip,
            helpers::{
                leb128_compute_last_byte_offset, leb128_compute_sn,
                leb128_compute_sn_recovered_at_position,
            },
        },
        sections::consts::LebParams,
        types::{
            AssignDeltaType, AssignValueType, Leb128BytesCountType, Leb128LengthType, LimitType,
            NewWbOffsetType, SectionLengthType, SharedState, Sn, WbOffsetType,
        },
    },
};

#[derive(Debug, Clone)]
pub struct LimitTypeFields<F> {
    pub is_limit_type: Column<Fixed>,
    pub is_limit_min: Column<Fixed>,
    pub is_limit_max: Column<Fixed>,
    pub limit_type_params_lt_chip: Rc<LtChip<F, 4>>,
    pub limit_type: Column<Advice>,
    pub limit_type_chip: Rc<BinaryNumberChip<F, LimitType, 8>>,
    pub is_limit_type_ctx: Column<Fixed>,
}

pub fn configure_constraints_for_q_first_and_q_last<F: Field>(
    cb: &mut BaseConstraintBuilder<F>,
    vc: &mut VirtualCells<F>,
    q_enable: &Column<Fixed>,
    q_first: &Column<Fixed>,
    q_first_column_selectors: &[Column<Fixed>],
    q_last: &Column<Fixed>,
    q_last_column_selectors: &[Column<Fixed>],
) {
    let q_enable_expr = vc.query_fixed(*q_enable, Rotation::cur());
    let q_first_expr = vc.query_fixed(*q_first, Rotation::cur());
    let not_q_first_expr = not::expr(q_first_expr.clone());
    let q_last_expr = vc.query_fixed(*q_last, Rotation::cur());
    let not_q_last_expr = not::expr(q_last_expr.clone());

    cb.require_boolean("q_first is boolean", q_first_expr.clone());
    cb.require_boolean("q_last is boolean", q_last_expr.clone());

    if q_first_column_selectors.len() > 0 {
        cb.condition(q_first_expr.clone(), |cb| {
            cb.require_equal(
                "q_first => specific selectors must be active",
                or::expr(
                    q_first_column_selectors
                        .iter()
                        .map(|&v| vc.query_fixed(v, Rotation::cur())),
                ),
                1.expr(),
            )
        });
    }
    if q_last_column_selectors.len() > 0 {
        cb.condition(q_last_expr.clone(), |cb| {
            cb.require_equal(
                "q_last => specific selectors must be active",
                or::expr(
                    q_last_column_selectors
                        .iter()
                        .map(|&v| vc.query_fixed(v, Rotation::cur())),
                ),
                1.expr(),
            )
        });
    }

    cb.condition(
        or::expr([q_first_expr.clone(), q_last_expr.clone()]),
        |cb| {
            cb.require_equal(
                "q_first || q_last => q_enable=1",
                q_enable_expr.clone(),
                1.expr(),
            );
        },
    );
    cb.condition(
        and::expr([q_first_expr.clone(), not_q_last_expr.clone()]),
        |cb| {
            let q_first_next_expr = vc.query_fixed(*q_first, Rotation::next());
            cb.require_zero(
                "q_first && !q_last -> !next.q_first",
                q_first_next_expr.clone(),
            );
        },
    );
    cb.condition(
        and::expr([q_last_expr.clone(), not_q_first_expr.clone()]),
        |cb| {
            let q_last_prev_expr = vc.query_fixed(*q_last, Rotation::prev());
            cb.require_zero(
                "q_last && !q_first -> !prev.q_last",
                q_last_prev_expr.clone(),
            );
        },
    );
    cb.condition(
        and::expr([not_q_first_expr.clone(), not_q_last_expr.clone()]),
        |cb| {
            let q_first_next_expr = vc.query_fixed(*q_first, Rotation::next());
            let q_last_prev_expr = vc.query_fixed(*q_last, Rotation::prev());
            cb.require_zero(
                "!q_first && !q_last -> !next.q_first",
                q_first_next_expr.clone(),
            );
            cb.require_zero(
                "!q_first && !q_last -> !prev.q_last",
                q_last_prev_expr.clone(),
            );
        },
    );
}

/// `is_check_next` is check next or prev
pub fn configure_transition_check<F: Field>(
    cb: &mut BaseConstraintBuilder<F>,
    vc: &mut VirtualCells<F>,
    name: &'static str,
    condition: Expression<F>,
    is_check_next: bool,
    columns_to_check: &[Column<Fixed>],
) {
    cb.condition(condition, |cb| {
        let mut lhs = 0.expr();
        for column_to_check in columns_to_check {
            lhs = lhs
                + vc.query_fixed(
                    *column_to_check,
                    Rotation(if is_check_next { 1 } else { -1 }),
                );
        }
        cb.require_equal(name, lhs, 1.expr())
    });
}

pub trait WasmLenPrefixedBytesSpanAwareChip<F: Field> {
    fn configure_len_prefixed_bytes_span_checks(
        cs: &mut ConstraintSystem<F>,
        leb128_chip: &LEB128Chip<F>,
        is_body: impl FnOnce(&mut VirtualCells<'_, F>) -> Expression<F>,
        body_item_rev_index: Column<Advice>,
        is_len_prefix: impl FnOnce(&mut VirtualCells<'_, F>) -> Expression<F>,
        is_last_item: impl FnOnce(&mut VirtualCells<'_, F>) -> Expression<F>,
    ) {
        cs.create_gate("len prefixed body gate", |vc| {
            let mut cb = BaseConstraintBuilder::default();

            let body_item_rev_index_expr = vc.query_advice(body_item_rev_index, Rotation::cur());
            let sn_expr = vc.query_advice(leb128_chip.config.sn, Rotation::cur());

            let is_len_prefix_expr = is_len_prefix(vc);
            let is_last_item_expr = is_last_item(vc);
            let is_body_expr = is_body(vc);

            cb.require_boolean("is_len_prefix is bool", is_len_prefix_expr.clone());
            cb.require_boolean("is_last_item is bool", is_last_item_expr.clone());

            cb.condition(is_len_prefix_expr.clone(), |cb| {
                cb.require_equal(
                    "len prefixed body starts from proper rev index",
                    body_item_rev_index_expr.clone(),
                    sn_expr.clone(),
                );
            });
            cb.condition(is_body_expr.clone(), |cb| {
                let body_item_rev_index_prev_expr =
                    vc.query_advice(body_item_rev_index, Rotation::prev());
                cb.require_equal(
                    "is_body => body_item_rev_index decreased by 1",
                    body_item_rev_index_prev_expr.clone() - 1.expr(),
                    body_item_rev_index_expr.clone(),
                );
            });
            cb.condition(is_last_item_expr.clone(), |cb| {
                cb.require_zero(
                    "is_last_item => body_item_rev_index=0",
                    body_item_rev_index_expr.clone(),
                );
            });

            cb.gate(or::expr([is_len_prefix_expr, is_body_expr]))
        });
    }
}

pub trait WasmCountPrefixedItemsAwareChip<F: Field> {
    fn configure_count_prefixed_items_checks(
        cs: &mut ConstraintSystem<F>,
        leb128_chip: &LEB128Chip<F>,
        body_item_rev_count: Column<Advice>,
        is_count_prefix: impl FnOnce(&mut VirtualCells<'_, F>) -> Expression<F>,
        is_body: impl FnOnce(&mut VirtualCells<'_, F>) -> Expression<F>,
        is_next_item: impl FnOnce(&mut VirtualCells<'_, F>) -> Expression<F>,
        is_last_item: impl FnOnce(&mut VirtualCells<'_, F>) -> Expression<F>,
    ) {
        cs.create_gate(
            "count prefixed items gate",
            |vc| {
                let mut cb = BaseConstraintBuilder::default();

                let body_item_rev_count_expr = vc.query_advice(body_item_rev_count, Rotation::cur());
                let sn_expr = vc.query_advice(leb128_chip.config.sn, Rotation::cur());

                let is_next_body_item_expr = is_next_item(vc);
                let is_count_prefix_expr = is_count_prefix(vc);
                let is_last_item_expr = is_last_item(vc);
                let is_body_expr = is_body(vc);

                cb.require_boolean("is_next_body_item is bool", is_next_body_item_expr.clone());
                cb.require_boolean("is_count_prefix is bool", is_count_prefix_expr.clone());
                cb.require_boolean("is_last_item is bool", is_last_item_expr.clone());
                cb.require_boolean("is_body is bool", is_body_expr.clone());

                cb.condition(
                    is_count_prefix_expr.clone(),
                    |cb| {
                        cb.require_equal(
                            "count prefixed items starts from proper rev count",
                            body_item_rev_count_expr.clone(),
                            sn_expr.clone(),
                        );
                    }
                );
                cb.condition(
                    is_next_body_item_expr.clone(),
                    |cb| {
                        let body_item_rev_count_prev_expr = vc.query_advice(body_item_rev_count, Rotation::prev());
                        cb.require_equal(
                            "is_next_body_item => prev.body_item_rev_count-1=body_item_rev_count",
                            body_item_rev_count_prev_expr.clone() - 1.expr(),
                            body_item_rev_count_expr.clone(),
                        );
                    }
                );
                cb.condition(
                    and::expr([
                        is_body_expr.clone(),
                        not::expr(is_next_body_item_expr.clone()),
                    ]),
                    |cb| {
                        let body_item_rev_count_prev_expr = vc.query_advice(body_item_rev_count, Rotation::prev());
                        cb.require_equal(
                            "is_body && !is_next_body_item => prev.body_item_rev_count=body_item_rev_count",
                            body_item_rev_count_prev_expr.clone(),
                            body_item_rev_count_expr.clone(),
                        );
                    }
                );
                cb.condition(
                    is_last_item_expr.clone(),
                    |cb| {
                        cb.require_zero(
                            "is_last_item => body_item_rev_count=0",
                            body_item_rev_count_expr.clone(),
                        );
                    }
                );

                cb.gate(or::expr([
                    is_count_prefix_expr,
                    is_body_expr,
                ]))
            }
        );
    }
}

pub trait WasmLimitTypeAwareChip<F: Field> {
    fn construct_limit_type_fields(
        cs: &mut ConstraintSystem<F>,
        q_enable: Column<Fixed>,
        leb128_chip: &LEB128Chip<F>,
    ) -> LimitTypeFields<F> {
        let is_limit_type = cs.fixed_column();
        let is_limit_min = cs.fixed_column();
        let is_limit_max = cs.fixed_column();
        let is_limit_type_ctx = cs.fixed_column();
        let limit_type = cs.advice_column();
        let config = BinaryNumberChip::configure(cs, is_limit_type_ctx, Some(limit_type.into()));
        let limit_type_chip = Rc::new(BinaryNumberChip::construct(config));

        let limit_type_params_lt_chip_config = LtChip::configure(
            cs,
            |vc| {
                and::expr([
                    vc.query_fixed(q_enable, Rotation::cur()),
                    limit_type_chip
                        .config
                        .value_equals(LimitType::MinMax, Rotation::cur())(vc),
                    vc.query_fixed(is_limit_min, Rotation::prev()),
                    vc.query_fixed(is_limit_max, Rotation::cur()),
                ])
            },
            |vc| vc.query_advice(leb128_chip.config.sn, Rotation::prev()),
            |vc| vc.query_advice(leb128_chip.config.sn, Rotation::cur()),
        );
        let limit_type_params_lt_chip =
            Rc::new(LtChip::construct(limit_type_params_lt_chip_config));

        LimitTypeFields {
            is_limit_type,
            is_limit_min,
            is_limit_max,
            limit_type_params_lt_chip,
            limit_type,
            limit_type_chip,
            is_limit_type_ctx,
        }
    }

    fn configure_limit_type_constraints(
        cs: &mut ConstraintSystem<F>,
        bytecode_table: &WasmBytecodeTable,
        q_enable: Column<Fixed>,
        leb128_chip: &LEB128Chip<F>,
        limit_type_fields: &LimitTypeFields<F>,
    ) {
        let LimitTypeFields {
            is_limit_type,
            is_limit_min,
            is_limit_max,
            limit_type_params_lt_chip,
            limit_type,
            limit_type_chip,
            is_limit_type_ctx,
        } = limit_type_fields;
        cs.create_gate("limit_type structure gate", |vc| {
            let mut cb = BaseConstraintBuilder::default();

            let q_enable_expr = vc.query_fixed(q_enable, Rotation::cur());

            let is_limit_type_expr = vc.query_fixed(*is_limit_type, Rotation::cur());
            let is_limit_min_expr = vc.query_fixed(*is_limit_min, Rotation::cur());
            let is_limit_max_expr = vc.query_fixed(*is_limit_max, Rotation::cur());

            let is_limit_type_ctx_expr = vc.query_fixed(*is_limit_type_ctx, Rotation::cur());

            let byte_val_expr = vc.query_advice(bytecode_table.value, Rotation::cur());
            let limit_type_prev_expr = vc.query_advice(*limit_type, Rotation::prev());
            let limit_type_expr = vc.query_advice(*limit_type, Rotation::cur());

            cb.require_boolean("is_limit_type is boolean", is_limit_type_expr.clone());
            cb.require_boolean(
                "is_limit_type_ctx is boolean",
                is_limit_type_ctx_expr.clone(),
            );

            cb.condition(is_limit_type_expr.clone(), |cb| {
                cb.require_in_set(
                    "limit_type => byte value is valid",
                    byte_val_expr.clone(),
                    vec![LimitType::MinOnly.expr(), LimitType::MinMax.expr()],
                )
            });
            cb.require_equal(
                "is_limit_type_ctx active on a specific flags only",
                is_limit_type_expr.clone() + is_limit_min_expr.clone() + is_limit_max_expr.clone(),
                is_limit_type_ctx_expr.clone(),
            );
            cb.condition(is_limit_type_expr.clone(), |cb| {
                cb.require_equal(
                    "is_limit_type => limit_type=byte_val",
                    limit_type_expr.clone(),
                    byte_val_expr.clone(),
                );
            });
            cb.condition(is_limit_type_ctx_expr.clone(), |cb| {
                let is_limit_type_ctx_prev_expr =
                    vc.query_fixed(*is_limit_type_ctx, Rotation::prev());
                cb.require_zero(
                    "is_limit_type_ctx && prev.is_limit_type_ctx => limit_type=prev.limit_type",
                    is_limit_type_ctx_prev_expr
                        * (limit_type_expr.clone() - limit_type_prev_expr.clone()),
                );
            });

            cb.gate(q_enable_expr.clone())
        });

        cs.create_gate("limit_type params are valid", |vc| {
            let mut cb = BaseConstraintBuilder::default();

            let limit_min_expr = vc.query_advice(leb128_chip.config.sn, Rotation::prev());
            let limit_max_expr = vc.query_advice(leb128_chip.config.sn, Rotation::cur());

            cb.condition(
                and::expr([
                    vc.query_fixed(q_enable, Rotation::cur()),
                    limit_type_chip
                        .config
                        .value_equals(LimitType::MinMax, Rotation::cur())(vc),
                    vc.query_fixed(*is_limit_min, Rotation::prev()),
                    vc.query_fixed(*is_limit_max, Rotation::cur()),
                ]),
                |cb| {
                    cb.require_zero(
                        "prev.limit_min <= limit_max",
                        (limit_type_params_lt_chip.config().is_lt(vc, None) - 1.expr())
                            * (limit_max_expr - limit_min_expr),
                    )
                },
            );

            cb.constraints
        });
    }
}

pub trait WasmSharedStateAwareChip<F: Field> {
    fn shared_state(&self) -> Rc<RefCell<SharedState>>;
}

pub trait WasmFuncCountAwareChip<F: Field>: WasmSharedStateAwareChip<F> {
    fn func_count_col(&self) -> Column<Advice>;

    fn assign_func_count(&self, region: &mut Region<F>, assign_offset: usize) -> Result<(), Error> {
        let func_count = self.shared_state().borrow().func_count;
        debug!("assign at {} func_count val {}", assign_offset, func_count);
        region
            .assign_advice(
                || {
                    format!(
                        "assign 'func_count' val {} at {}",
                        func_count, assign_offset
                    )
                },
                self.func_count_col(),
                assign_offset,
                || Value::known(F::from(func_count as u64)),
            )
            .map_err(remap_error_to_assign_at(assign_offset))?;

        Ok(())
    }
}

pub trait WasmErrorAwareChip<F: Field>: WasmSharedStateAwareChip<F> {
    fn error_code_col(&self) -> Column<Advice>;

    fn configure_error_code(
        cs: &mut ConstraintSystem<F>,
        q_enable: Column<Fixed>,
        q_first: Column<Fixed>,
        q_last: Column<Fixed>,
        error_code: Column<Advice>,
    ) {
        cs.create_gate("ErrorCode gate", |vc| {
            let mut cb = BaseConstraintBuilder::default();

            let q_enable_expr = vc.query_fixed(q_enable, Rotation::cur());
            let q_first_expr = vc.query_fixed(q_first, Rotation::cur());
            let not_q_first_expr = not::expr(q_first_expr.clone());
            let q_last_expr = vc.query_fixed(q_last, Rotation::cur());
            let not_q_last_expr = not::expr(q_last_expr.clone());
            let error_code_expr = vc.query_advice(error_code, Rotation::cur());

            cb.require_boolean("error_code is bool", error_code_expr.clone());

            cb.condition(
                and::expr([not_q_first_expr.clone(), not::expr(error_code_expr.clone())]),
                |cb| {
                    let error_code_prev_expr = vc.query_advice(error_code, Rotation::prev());
                    cb.require_equal(
                        "error_code=0 => prev.error_code=0",
                        error_code_expr.clone(),
                        error_code_prev_expr.clone(),
                    );
                },
            );
            cb.condition(
                and::expr([not_q_last_expr.clone(), error_code_expr.clone()]),
                |cb| {
                    let error_code_next_expr = vc.query_advice(error_code, Rotation::next());
                    cb.require_equal(
                        "error_code=1 => next.error_code=1",
                        error_code_expr.clone(),
                        error_code_next_expr.clone(),
                    );
                },
            );

            cb.gate(q_enable_expr)
        });
    }

    fn assign_error_code(
        &self,
        region: &mut Region<F>,
        assign_offset: usize,
        error_code_replacer: Option<u64>,
    ) -> Result<(), Error> {
        let error_code = error_code_replacer.unwrap_or(self.shared_state().borrow().error_code);
        debug!("assign at {} error_code val {}", assign_offset, error_code);
        region
            .assign_advice(
                || {
                    format!(
                        "assign 'error_code' val {} at {}",
                        error_code, assign_offset
                    )
                },
                self.error_code_col(),
                assign_offset,
                || Value::known(F::from(error_code)),
            )
            .map_err(remap_error_to_assign_at(assign_offset))?;
        Ok(())
    }

    fn assign_error_code_rest(
        &self,
        region: &mut Region<F>,
        assign_offset: usize,
        len: usize,
        explicit_error_code: Option<u64>,
    ) -> Result<(), Error> {
        let error_code = explicit_error_code.unwrap_or(self.shared_state().borrow().error_code);
        for offset in assign_offset..assign_offset + len {
            debug!("assign at {} error_code val {}", offset, error_code);
            region
                .assign_advice(
                    || format!("assign 'error_code' val {} at {}", error_code, offset),
                    self.error_code_col(),
                    offset,
                    || Value::known(F::from(error_code)),
                )
                .map_err(remap_error_to_assign_at(offset))?;
        }
        Ok(())
    }

    fn get_selector_expr_enriched_with_error_processing(
        vc: &mut VirtualCells<F>,
        q_enable: Column<Fixed>,
        shared_state: &SharedState,
        error_code: Column<Advice>,
    ) -> Expression<F> {
        let q_enable_expr = vc.query_fixed(q_enable, Rotation::cur());
        let error_code_expr = vc.query_advice(error_code, Rotation::cur());
        let not_error_code_expr = not::expr(error_code_expr);

        q_enable_expr
            * if shared_state.error_processing_enabled {
                not_error_code_expr
            } else {
                1.expr()
            }
    }
}

pub trait WasmBytecodeNumberAwareChip<F: Field>: WasmSharedStateAwareChip<F> {
    fn bytecode_number_col(&self) -> Column<Advice>;
    fn configure_bytecode_number(
        cs: &mut ConstraintSystem<F>,
        q_enable: Column<Fixed>,
        q_first: Column<Fixed>,
        q_last: Column<Fixed>,
        bytecode_number: Column<Advice>,
    ) {
        cs.create_gate("bytecode-number gate", |vc| {
            let mut cb = BaseConstraintBuilder::default();

            let q_enable_expr = vc.query_fixed(q_enable, Rotation::cur());
            let q_first_expr = vc.query_fixed(q_first, Rotation::cur());
            let q_first_next_expr = vc.query_fixed(q_first, Rotation::next());
            let not_q_first_expr = not::expr(q_first_expr.clone());
            let q_last_expr = vc.query_fixed(q_last, Rotation::cur());
            let bytecode_number_expr = vc.query_advice(bytecode_number, Rotation::cur());

            cb.condition(not_q_first_expr.clone(), |cb| {
                let q_enable_prev_expr = vc.query_fixed(q_enable, Rotation::prev());
                let bytecode_number_prev_expr = vc.query_advice(bytecode_number, Rotation::prev());
                cb.require_equal(
                    "not_q_first => bytecode_number=prev.bytecode_number",
                    q_enable_prev_expr.clone() * bytecode_number_expr.clone(),
                    q_enable_prev_expr * bytecode_number_prev_expr.clone(),
                );
            });
            cb.condition(
                and::expr([q_last_expr.clone(), q_first_next_expr.clone()]),
                |cb| {
                    let bytecode_number_next_expr =
                        vc.query_advice(bytecode_number, Rotation::next());
                    cb.require_equal(
                        "q_last && next.q_first => bytecode_number=next.bytecode_number-1",
                        bytecode_number_expr.clone(),
                        bytecode_number_next_expr.clone() - 1.expr(),
                    );
                },
            );

            cb.gate(q_enable_expr)
        });
    }

    fn assign_bytecode_number(
        &self,
        region: &mut Region<F>,
        assign_offset: usize,
        explicit_bytecode_number: Option<u64>,
    ) -> Result<(), Error> {
        let bytecode_number =
            explicit_bytecode_number.unwrap_or(self.shared_state().borrow().bytecode_number);
        debug!(
            "assign at {} bytecode_number val {}",
            assign_offset, bytecode_number
        );
        region
            .assign_advice(
                || {
                    format!(
                        "assign 'bytecode_number' val {} at {}",
                        bytecode_number, assign_offset
                    )
                },
                self.bytecode_number_col(),
                assign_offset,
                || Value::known(F::from(bytecode_number)),
            )
            .map_err(remap_error_to_assign_at(assign_offset))?;

        Ok(())
    }
}

pub trait WasmBlockLevelAwareChip<F: Field>: WasmSharedStateAwareChip<F> {
    fn block_level_col(&self) -> Column<Advice>;

    fn assign_block_level(
        &self,
        region: &mut Region<F>,
        assign_offset: usize,
    ) -> Result<(), Error> {
        let block_level = self.shared_state().borrow().block_level;
        debug!(
            "assign at {} block_level val {}",
            assign_offset, block_level
        );
        region
            .assign_advice(
                || {
                    format!(
                        "assign 'block_level' val {} at {}",
                        block_level, assign_offset
                    )
                },
                self.block_level_col(),
                assign_offset,
                || Value::known(F::from(block_level as u64)),
            )
            .map_err(remap_error_to_assign_at(assign_offset))?;
        Ok(())
    }
}

pub trait WasmAssignAwareChip<F: Field> {
    type AssignType;

    fn assign(
        &self,
        region: &mut Region<F>,
        wb: &WasmBytecode,
        wb_offset: WbOffsetType,
        assign_delta: AssignDeltaType,
        assign_types: &[Self::AssignType],
        assign_value: AssignValueType,
        leb_params: Option<LebParams>,
    ) -> Result<(), Error> {
        validate_wb_offset(wb, wb_offset)?;
        self.assign_internal(
            region,
            wb,
            wb_offset,
            assign_delta,
            assign_types,
            assign_value,
            leb_params,
        )
    }

    fn assign_internal(
        &self,
        region: &mut Region<F>,
        wb: &WasmBytecode,
        wb_offset: WbOffsetType,
        assign_delta: AssignDeltaType,
        assign_types: &[Self::AssignType],
        assign_value: AssignValueType,
        leb_params: Option<LebParams>,
    ) -> Result<(), Error>;
}

pub trait WasmMarkupLeb128SectionAwareChip<F: Field>: WasmAssignAwareChip<F> {
    fn markup_leb_section(
        &self,
        region: &mut Region<F>,
        wb: &WasmBytecode,
        wb_offset: usize,
        assign_delta: AssignDeltaType,
        assign_types: &[Self::AssignType],
    ) -> Result<(Sn, Leb128LengthType), Error> {
        let is_signed = false;
        let (sn, last_byte_offset) =
            leb128_compute_sn(wb.bytes.as_slice(), is_signed, wb_offset)
                .map_err(remap_error_to_compute_value_at(wb_offset + assign_delta))?;
        let mut sn_recovered_at_pos = 0;
        let last_byte_rel_offset = last_byte_offset - wb_offset;
        for byte_rel_offset in 0..=last_byte_rel_offset {
            let wb_offset = wb_offset + byte_rel_offset;
            sn_recovered_at_pos = leb128_compute_sn_recovered_at_position(
                sn_recovered_at_pos,
                is_signed,
                byte_rel_offset,
                last_byte_rel_offset,
                wb.bytes[wb_offset],
            );
            let leb_params = Some(LebParams {
                is_signed,
                byte_rel_offset,
                last_byte_rel_offset,
                sn,
                sn_recovered_at_pos,
            });
            self.assign(
                region,
                wb,
                wb_offset,
                assign_delta,
                assign_types,
                1,
                leb_params,
            )?;
        }

        Ok((sn, last_byte_rel_offset + 1))
    }
}

pub trait WasmBytesAwareChip<F: Field>: WasmAssignAwareChip<F> {
    fn markup_bytes_section(
        &self,
        region: &mut Region<F>,
        wb: &WasmBytecode,
        assign_types: &[Self::AssignType],
        wb_offset: usize,
        assign_delta: AssignDeltaType,
        len: usize,
    ) -> Result<NewWbOffsetType, Error> {
        let offset_end = wb_offset + len;
        if offset_end >= wb.bytes.len() {
            return Err(error_index_out_of_bounds(wb_offset));
        }
        for offset in wb_offset..offset_end {
            self.assign(
                region,
                wb,
                offset,
                offset + assign_delta,
                assign_types,
                1,
                None,
            )?;
        }
        Ok(wb_offset + len)
    }
}

pub trait WasmNameAwareChip<F: Field>: WasmAssignAwareChip<F> {
    fn markup_name_section(
        &self,
        region: &mut Region<F>,
        wb: &WasmBytecode,
        wb_offset: usize,
        assign_delta: AssignDeltaType,
        assign_types: &[Self::AssignType],
        name_len: usize,
        assign_value: AssignValueType,
    ) -> Result<NewWbOffsetType, Error> {
        let offset_end = wb_offset + name_len;
        if offset_end >= wb.bytes.len() {
            return Err(error_index_out_of_bounds(wb_offset));
        }
        for offset in wb_offset..offset_end {
            self.assign(
                region,
                wb,
                offset,
                assign_delta,
                assign_types,
                assign_value,
                None,
            )?;
        }
        Ok(wb_offset + name_len)
    }
}

pub fn digit_char_to_number(ch: &char) -> u8 {
    *ch as u8 - 48
}

pub fn wasm_compute_section_len(
    wb: &[u8],
    len_start_index: usize,
) -> Result<(SectionLengthType, Leb128BytesCountType), Error> {
    let mut section_len: usize = 0;
    let mut i = len_start_index;
    loop {
        let byte = wb.get(i).ok_or(Error::IndexOutOfBoundsSimple)?;
        let mut byte_val: u32 = (byte & 0b1111111) as u32;
        let pow = checked_pow(0b10000000, i - len_start_index).ok_or(Error::ComputationFailed)?;
        byte_val = byte_val * pow;
        section_len += byte_val as usize;
        if byte & 0b10000000 == 0 {
            break;
        }
        i += 1;
        if i - len_start_index >= MAX_LEB128_BYTES {
            return Err(Error::Leb128MaxBytes);
        }
    }
    Ok((section_len, (i - len_start_index + 1) as u8))
}

#[cfg(any(feature = "test", test))]
pub fn wat_extract_section_bytecode(path_to_file: &str, kind: Kind) -> Vec<u8> {
    let wat: Vec<u8> = std::fs::read(path_to_file).unwrap();
    let wasm_binary = wat2wasm(&wat.clone()).unwrap();

    let mut m = Module::decode_from(wasm_binary.as_slice()).unwrap();
    let mut bytes = Vec::<u8>::new();
    for s in m.sections.iter_mut() {
        if s.kind() == kind {
            wasmbin_unlazify_with_opt(s, false).unwrap();
            s.encode(&mut bytes).unwrap();
            break;
        }
    }

    return bytes;
}

#[cfg(any(feature = "test", test))]
pub fn wat_extract_section_body_bytecode(path_to_file: &str, kind: Kind) -> Vec<u8> {
    let bytecode = &wat_extract_section_bytecode(path_to_file, kind)[..];
    if bytecode.len() <= 0 {
        return vec![];
    }
    let last_byte_offset = leb128_compute_last_byte_offset(bytecode, 1).unwrap();
    return bytecode[last_byte_offset + 1..].to_vec();
}

#[cfg(any(feature = "test", test))]
pub fn wasmbin_unlazify_with_opt<T: Visit>(
    wasm: &mut T,
    include_raw: bool,
) -> Result<(), DecodeError> {
    let res = if include_raw {
        wasm.visit(|()| {})
    } else {
        wasm.visit_mut(|()| {})
    };
    match res {
        Ok(()) => Ok(()),
        Err(err) => match err {
            VisitError::LazyDecode(err) => Err(err),
            VisitError::Custom(err) => match err {},
        },
    }
}
