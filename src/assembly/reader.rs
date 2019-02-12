// Copyright (c) 2017 Fabian Schuiki
#![allow(dead_code, unused_imports)]

use crate::inst::*;
use crate::konst;
use crate::ty::*;
use crate::{
    assembly::Writer, Aggregate, Argument, ArrayAggregate, Block, BlockPosition, BlockRef, Entity,
    Function, Module, Process, SeqBody, StructAggregate, Value, ValueRef, Visitor,
};
use combine::char::{alpha_num, digit, space, string, Spaces};
use combine::combinator::{Expected, FnParser, Skip};
use combine::*;
use num::{BigInt, BigRational};
use std;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::marker::PhantomData;
use std::rc::Rc;

pub fn parse_str(input: &str) -> Result<Module, String> {
    match parser(module).parse(State::new(input)) {
        Ok((m, _)) => Ok(m),
        Err(err) => Err(format!("{}", err)),
    }
}

/// Applies the inner parser `p` and skips any trailing spaces.
fn lex<P>(p: P) -> Skip<P, Whitespace<P::Input>>
where
    P: Parser,
    P::Input: Stream<Item = char>,
{
    p.skip(Whitespace {
        _marker: PhantomData,
    })
}

struct Whitespace<I> {
    _marker: PhantomData<I>,
}

impl<I: Stream<Item = char>> Parser for Whitespace<I> {
    type Input = I;
    type Output = ();

    fn parse_stream(&mut self, input: I) -> ParseResult<(), I> {
        whitespace(input)
    }
}

/// Skip spaces (not line breaks).
fn whitespace<I>(input: I) -> ParseResult<(), I>
where
    I: Stream<Item = char>,
{
    skip_many(satisfy(|c: char| c.is_whitespace() && c != '\n')).parse_stream(input)
}

/// Skip whitespace and comments.
fn leading_whitespace<I>(input: I) -> ParseResult<(), I>
where
    I: Stream<Item = char>,
{
    let comment = (token(';'), skip_many(satisfy(|c| c != '\n'))).map(|_| ());
    skip_many(skip_many1(space()).or(comment)).parse_stream(input)
}

/// Parse the part of a name after the '@' or '%' introducing it.
fn inner_name<I>(input: I) -> ParseResult<String, I>
where
    I: Stream<Item = char>,
{
    many1(alpha_num().or(token('_')).or(token('.'))).parse_stream(input)
}

/// Parse a global or local name, e.g. `@foo` or `%bar` respectively.
fn name<I>(input: I) -> ParseResult<(bool, String), I>
where
    I: Stream<Item = char>,
{
    (
        token('%').map(|_| false).or(token('@').map(|_| true)),
        parser(inner_name),
    )
        .parse_stream(input)
}

/// Parse a local name, e.g. `%bar`.
fn local_name<I>(input: I) -> ParseResult<String, I>
where
    I: Stream<Item = char>,
{
    (token('%'), parser(inner_name))
        .map(|(_, s)| s)
        .parse_stream(input)
}

/// Parse a type.
fn ty_parser<I>(input: I) -> ParseResult<Type, I>
where
    I: Stream<Item = char>,
{
    enum Suffix {
        Pointer,
        Signal,
    }

    let int = |input| {
        many1(digit())
            .map(|s: String| s.parse::<usize>().unwrap())
            .parse_stream(input)
    };
    choice!(
        string("void").map(|_| void_ty()),
        string("time").map(|_| time_ty()),
        token('i').with(parser(&int)).map(|i| int_ty(i)),
        token('n').with(parser(&int)).map(|i| enum_ty(i)),
        lex(token('{'))
            .with(sep_by(lex(parser(ty_parser)), lex(token(','))))
            .skip(token('}'))
            .map(|v| struct_ty(v)),
        lex(token('['))
            .with((lex(parser(&int)), lex(token('x')), parser(ty_parser)))
            .skip(token(']'))
            .map(|(s, _, t)| array_ty(s, t))
    )
    .and(optional(choice!(
        token('*').map(|_| Suffix::Pointer),
        token('$').map(|_| Suffix::Signal)
    )))
    .map(|(ty, suffix)| match suffix {
        Some(Suffix::Pointer) => pointer_ty(ty),
        Some(Suffix::Signal) => signal_ty(ty),
        None => ty,
    })
    .parse_stream(input)
}

/// Parse the end of a line, with an optional comment.
fn eol<I>(input: I) -> ParseResult<(), I>
where
    I: Stream<Item = char>,
{
    let comment = (token(';'), skip_many(satisfy(|c| c != '\n'))).map(|_| ());
    parser(whitespace)
        .skip(optional(comment))
        .skip(token('\n').map(|_| ()).or(eof()))
        .skip(parser(leading_whitespace))
        .expected("end of line")
        .parse_stream(input)
}

/// Parse a sequence of basic blocks.
fn blocks<I>(ctx: &NameTable, input: I) -> ParseResult<Vec<(Block, Vec<Inst>)>, I>
where
    I: Stream<Item = char>,
{
    let block = parser(local_name)
        .skip(token(':'))
        .skip(parser(eol))
        .expected("basic block")
        .and(env_parser(ctx, insts))
        .map(|(name, insts)| (ctx.declare_block(name), insts));
    many(block).parse_stream(input)
}

/// Parse a sequence of instructions.
fn insts<I>(ctx: &NameTable, input: I) -> ParseResult<Vec<Inst>, I>
where
    I: Stream<Item = char>,
{
    let name = parser(local_name)
        .skip(parser(whitespace))
        .skip(token('='))
        .skip(parser(whitespace));
    let inst = choice!(
        r#try(env_parser(ctx, unary_inst)),
        r#try(env_parser(ctx, binary_inst)),
        r#try(env_parser(ctx, compare_inst)),
        r#try(env_parser(ctx, call_inst)),
        r#try(env_parser(ctx, instance_inst)),
        r#try(env_parser(ctx, wait_inst)),
        r#try(env_parser(ctx, return_inst)),
        r#try(env_parser(ctx, branch_inst)),
        r#try(env_parser(ctx, signal_inst)),
        r#try(env_parser(ctx, probe_inst)),
        r#try(env_parser(ctx, drive_inst)),
        r#try(env_parser(ctx, variable_inst)),
        r#try(env_parser(ctx, load_inst)),
        r#try(env_parser(ctx, store_inst)),
        r#try(env_parser(ctx, insert_inst)),
        r#try(env_parser(ctx, extract_inst)),
        r#try(env_parser(ctx, shift_inst)),
        r#try(string("halt").map(|_| InstKind::HaltInst))
    );
    let named_inst = r#try(optional(name))
        .and(inst)
        .skip(parser(eol))
        .map(|(name, inst)| {
            let inst = Inst::new(name.clone().and_then(untemp_name), inst);
            if let Some(name) = name {
                ctx.insert(NameKey(false, name), inst.as_ref().into(), inst.ty());
            }
            inst
        });
    many(named_inst).parse_stream(input)
}

/// Parse a unary instruction.
fn unary_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    let unary_op = choice!(string("not").map(|_| UnaryOp::Not));

    // Parse the operator and type.
    let ((op, ty), consumed) = lex(unary_op)
        .and(lex(parser(ty_parser)))
        .parse_stream(input)?;

    // Parse the operand, passing in the type as context.
    let (arg, consumed) =
        consumed.combine(|input| env_parser((ctx, &ty), inline_value_infer).parse_stream(input))?;

    Ok((InstKind::UnaryInst(op, ty, arg), consumed))
}

/// Parse a binary instruction.
fn binary_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    let binary_op = choice!(
        r#try(string("add").map(|_| BinaryOp::Add)),
        r#try(string("sub").map(|_| BinaryOp::Sub)),
        r#try(string("mul").map(|_| BinaryOp::Mul)),
        r#try(string("div").map(|_| BinaryOp::Div)),
        r#try(string("mod").map(|_| BinaryOp::Mod)),
        r#try(string("rem").map(|_| BinaryOp::Rem)),
        r#try(string("and").map(|_| BinaryOp::And)),
        r#try(string("or").map(|_| BinaryOp::Or)),
        r#try(string("xor").map(|_| BinaryOp::Xor))
    );

    // Parse the operator and type.
    let ((op, ty), consumed) = lex(binary_op)
        .and(lex(parser(ty_parser)))
        .parse_stream(input)?;

    // Parse the left and right hand side, passing in the type as context.
    let ((lhs, rhs), consumed) = consumed.combine(|input| {
        (
            lex(env_parser((ctx, &ty), inline_value_infer)),
            env_parser((ctx, &ty), inline_value_infer),
        )
            .parse_stream(input)
    })?;

    Ok((InstKind::BinaryInst(op, ty, lhs, rhs), consumed))
}

/// Parse a compare instruction.
fn compare_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    let compare_op = choice!(
        r#try(string("eq").map(|_| CompareOp::Eq)),
        r#try(string("neq").map(|_| CompareOp::Neq)),
        r#try(string("slt").map(|_| CompareOp::Slt)),
        r#try(string("sgt").map(|_| CompareOp::Sgt)),
        r#try(string("sle").map(|_| CompareOp::Sle)),
        r#try(string("sge").map(|_| CompareOp::Sge)),
        r#try(string("ult").map(|_| CompareOp::Ult)),
        r#try(string("ugt").map(|_| CompareOp::Ugt)),
        r#try(string("ule").map(|_| CompareOp::Ule)),
        r#try(string("uge").map(|_| CompareOp::Uge))
    );

    // Parse the operator and type.
    let ((op, ty), consumed) = lex(string("cmp"))
        .with(lex(compare_op))
        .and(lex(parser(ty_parser)))
        .parse_stream(input)?;

    // Parse the left and right hand side, passing in the type as context.
    let ((lhs, rhs), consumed) = consumed.combine(|input| {
        (
            lex(env_parser((ctx, &ty), inline_value_infer)),
            env_parser((ctx, &ty), inline_value_infer),
        )
            .parse_stream(input)
    })?;

    Ok((InstKind::CompareInst(op, ty, lhs, rhs), consumed))
}

/// Parse a call instruction.
fn call_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    let ((global, name), consumed) = lex(string("call"))
        .with(lex(parser(name)))
        .parse_stream(input)?;
    let (target, ty) = ctx.lookup(&NameKey(global, name));
    let (args, consumed) = {
        let mut arg_tys = ty.unwrap_func().0.into_iter();
        let (args, consumed) = consumed.combine(|input| {
            between(
                lex(token('(')),
                token(')'),
                sep_by(
                    parser(|input| {
                        env_parser(
                            (ctx, arg_tys.next().expect("missing argument")),
                            inline_value_infer,
                        )
                        .parse_stream(input)
                    }),
                    lex(token(',')),
                ),
            )
            .parse_stream(input)
        })?;
        (args, consumed)
    };
    Ok(((InstKind::CallInst(ty, target, args)), consumed))
}

/// Parse an instance instruction.
fn instance_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    let ((global, name), consumed) = lex(string("inst"))
        .with(lex(parser(name)))
        .parse_stream(input)?;
    let (target, ty) = ctx.lookup(&NameKey(global, name));
    let (ins, outs, consumed) = {
        let (in_tys, out_tys) = ty.unwrap_entity();

        let mut arg_tys = in_tys.into_iter();
        let (ins, consumed) = consumed.combine(|input| {
            r#try(
                // This try block is necessary since otherwise in the case of an
                // empty argument list, the inline_value_infer parser would be applied
                // to see if any arguments are present. However, this causes a
                // panic since arg_tys is empty. Therefore we have to treat the
                // empty argument list as a special case.
                lex(token('(')).and(lex(token(')'))).map(|_| Vec::new()),
            )
            .or(between(
                lex(token('(')),
                lex(token(')')),
                sep_by(
                    parser(|input| {
                        env_parser(
                            (ctx, arg_tys.next().expect("missing argument")),
                            inline_value_infer,
                        )
                        .parse_stream(input)
                    }),
                    lex(token(',')),
                ),
            ))
            .parse_stream(input)
        })?;

        let mut arg_tys = out_tys.into_iter();
        let (outs, consumed) = consumed.combine(|input| {
            r#try(
                // This try block is necessary since otherwise in the case of an
                // empty argument list, the inline_value_infer parser would be applied
                // to see if any arguments are present. However, this causes a
                // panic since arg_tys is empty. Therefore we have to treat the
                // empty argument list as a special case.
                lex(token('(')).and(lex(token(')'))).map(|_| Vec::new()),
            )
            .or(between(
                lex(token('(')),
                token(')'),
                sep_by(
                    parser(|input| {
                        env_parser(
                            (ctx, arg_tys.next().expect("missing argument")),
                            inline_value_infer,
                        )
                        .parse_stream(input)
                    }),
                    lex(token(',')),
                ),
            ))
            .parse_stream(input)
        })?;

        (ins, outs, consumed)
    };
    Ok(((InstKind::InstanceInst(ty, target, ins, outs)), consumed))
}

/// Parse a wait instruction.
fn wait_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    (
        lex(string("wait")).with(env_parser(ctx, inline_label)),
        optional(
            r#try(parser(whitespace).skip(lex(string("for"))))
                .with(env_parser((ctx, &time_ty()), inline_value_infer)),
        ),
        many(
            r#try(parser(whitespace).skip(lex(token(','))))
                .with(env_parser(ctx, inline_named_value))
                .map(|(v, _)| v),
        ),
    )
        .map(|(target, time, signals)| InstKind::WaitInst(target, time, signals))
        .parse_stream(input)
}

/// Parse a return instruction.
fn return_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    string("ret")
        .with(optional(r#try(
            parser(whitespace)
                .with(parser(ty_parser))
                .skip(parser(whitespace))
                .then(|ty| {
                    parser(move |input| {
                        let (value, consumed) =
                            env_parser((ctx, &ty), inline_value_infer).parse_stream(input)?;
                        Ok(((ty.clone(), value), consumed))
                    })
                }),
        )))
        .map(|v| match v {
            Some((ty, value)) => InstKind::ReturnInst(ReturnKind::Value(ty, value)),
            None => InstKind::ReturnInst(ReturnKind::Void),
        })
        .parse_stream(input)
}

/// Parse a branch instruction.
fn branch_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    lex(string("br"))
        .with(choice!(
            lex(string("label"))
                .with(env_parser(ctx, inline_label))
                .map(|v| InstKind::BranchInst(BranchKind::Uncond(v))),
            (
                lex(env_parser((ctx, &int_ty(1)), inline_value_infer)).skip(lex(string("label"))),
                lex(env_parser(ctx, inline_label)),
                env_parser(ctx, inline_label),
            )
                .map(|(c, t, f)| InstKind::BranchInst(BranchKind::Cond(c, t, f)))
        ))
        .parse_stream(input)
}

/// Parse a signal instruction.
fn signal_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    lex(string("sig"))
        .with(parser(ty_parser))
        .then(|ty| {
            parser(move |input| {
                let (value, consumed) = optional(r#try(
                    parser(whitespace).with(env_parser((ctx, &ty), inline_value_infer)),
                ))
                .parse_stream(input)?;
                Ok(((ty.clone(), value), consumed))
            })
        })
        .map(|(ty, init)| InstKind::SignalInst(ty, init))
        .parse_stream(input)
}

/// Parse a probe instruction.
fn probe_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    let ((signal, ty), consumed) = lex(string("prb"))
        .with(env_parser(ctx, inline_named_value))
        .parse_stream(input)?;
    Ok((
        InstKind::ProbeInst(ty.unwrap_signal().clone(), signal),
        consumed,
    ))
}

/// Parse a drive instruction.
fn drive_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    let ((signal, ty), consumed) = lex(string("drv"))
        .with(lex(env_parser(ctx, inline_named_value)))
        .parse_stream(input)?;

    let ((value, delay), consumed) = consumed.combine(|input| {
        env_parser((ctx, ty.unwrap_signal()), inline_value_infer)
            .and(optional(r#try(
                parser(whitespace).with(env_parser((ctx, &time_ty()), inline_value_infer)),
            )))
            .parse_stream(input)
    })?;

    Ok((InstKind::DriveInst(signal, value, delay), consumed))
}

/// Parse a variable instruction.
fn variable_inst<I>(_ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    lex(string("var"))
        .with(parser(ty_parser))
        .map(|ty| InstKind::VariableInst(ty))
        .parse_stream(input)
}

/// Parse a load instruction.
fn load_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    lex(string("load"))
        .with(parser(ty_parser))
        .then(|ty| {
            parser(move |input| {
                let (value, consumed) = parser(whitespace)
                    .with(env_parser((ctx, &ty), inline_value_infer))
                    .parse_stream(input)?;
                Ok(((ty.clone(), value), consumed))
            })
        })
        .map(|(ty, ptr)| InstKind::LoadInst(ty, ptr))
        .parse_stream(input)
}

/// Parse a store instruction.
fn store_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    lex(string("store"))
        .with(parser(ty_parser))
        .then(|ty| {
            parser(move |input| {
                let ((ptr, value), consumed) = (
                    parser(whitespace).with(env_parser((ctx, &ty), inline_value_infer)),
                    parser(whitespace).with(env_parser((ctx, &ty), inline_value_infer)),
                )
                    .parse_stream(input)?;
                Ok(((ty.clone(), ptr, value), consumed))
            })
        })
        .map(|(ty, ptr, value)| InstKind::StoreInst(ty, ptr, value))
        .parse_stream(input)
}

/// Parse an insert instruction.
fn insert_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    let element_variant = lex(string("element"))
        .with(lex(parser(ty_parser)))
        .then(|ty| {
            parser(move |input| {
                let ((ptr, _, index), consumed) = (
                    lex(env_parser((ctx, &ty), inline_value_infer)),
                    lex(token(',')),
                    many1(digit()).map(|s: String| s.parse().unwrap()),
                )
                    .parse_stream(input)?;
                Ok(((ty.clone(), ptr, SliceMode::Element(index)), consumed))
            })
        });
    let slice_variant = lex(string("slice"))
        .with(lex(parser(ty_parser)))
        .then(|ty| {
            parser(move |input| {
                let ((ptr, _, base, _, length), consumed) = (
                    lex(env_parser((ctx, &ty), inline_value_infer)),
                    lex(token(',')),
                    many1(digit()).map(|s: String| s.parse().unwrap()),
                    lex(token(',')),
                    many1(digit()).map(|s: String| s.parse().unwrap()),
                )
                    .parse_stream(input)?;
                Ok(((ty.clone(), ptr, SliceMode::Slice(base, length)), consumed))
            })
        });
    lex(string("insert"))
        .with((
            lex(choice!(r#try(element_variant), r#try(slice_variant))),
            lex(token(',')),
            env_parser((ctx, None), inline_value),
        ))
        .map(|((ty, ptr, mode), _, (value, _))| InstKind::InsertInst(ty, ptr, mode, value))
        .parse_stream(input)
}

/// Parse an extract instruction.
fn extract_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    let element_variant = lex(string("element"))
        .with(lex(parser(ty_parser)))
        .then(|ty| {
            parser(move |input| {
                let ((ptr, _, index), consumed) = (
                    lex(env_parser((ctx, &ty), inline_value_infer)),
                    lex(token(',')),
                    many1(digit()).map(|s: String| s.parse().unwrap()),
                )
                    .parse_stream(input)?;
                Ok(((ty.clone(), ptr, SliceMode::Element(index)), consumed))
            })
        });
    let slice_variant = lex(string("slice"))
        .with(lex(parser(ty_parser)))
        .then(|ty| {
            parser(move |input| {
                let ((ptr, _, base, _, length), consumed) = (
                    lex(env_parser((ctx, &ty), inline_value_infer)),
                    lex(token(',')),
                    many1(digit()).map(|s: String| s.parse().unwrap()),
                    lex(token(',')),
                    many1(digit()).map(|s: String| s.parse().unwrap()),
                )
                    .parse_stream(input)?;
                Ok(((ty.clone(), ptr, SliceMode::Slice(base, length)), consumed))
            })
        });
    lex(string("extract"))
        .with(choice!(r#try(element_variant), r#try(slice_variant)))
        .map(|(ty, ptr, mode)| InstKind::ExtractInst(ty, ptr, mode))
        .parse_stream(input)
}

/// Parse a shift instruction.
fn shift_inst<I>(ctx: &NameTable, input: I) -> ParseResult<InstKind, I>
where
    I: Stream<Item = char>,
{
    let fields = (
        lex(choice!(
            r#try(string("shl")).map(|_| ShiftDir::Left),
            r#try(string("shr")).map(|_| ShiftDir::Right)
        )),
        lex(env_parser(ctx, inline_value_explicit)),
        lex(token(',')),
        lex(env_parser(ctx, inline_value_standalone)),
        lex(token(',')),
        env_parser(ctx, inline_value_standalone),
    );
    let mut inst = fields.map(|(dir, (target, ty), _, insert, _, amount)| {
        InstKind::ShiftInst(dir, ty, target, insert, amount)
    });
    inst.parse_stream(input)
}

/// Parse an inline value which may infer its type from context.
fn inline_value_infer<I>((ctx, ty): (&NameTable, &Type), input: I) -> ParseResult<ValueRef, I>
where
    I: Stream<Item = char>,
{
    inline_value((ctx, Some(ty)), input).map(|((v, _), c)| (v, c))
}

/// Parse an inline value which has a self-determined type.
fn inline_value_standalone<I>(ctx: &NameTable, input: I) -> ParseResult<ValueRef, I>
where
    I: Stream<Item = char>,
{
    inline_value((ctx, None), input).map(|((v, _), c)| (v, c))
}

/// Parse an inline value with an explicitly stated type.
fn inline_value_explicit<I>(ctx: &NameTable, input: I) -> ParseResult<(ValueRef, Type), I>
where
    I: Stream<Item = char>,
{
    // inline_value((ctx, None), input).map(|((v, _), c)| (v, c))
    lex(parser(ty_parser))
        .then(|ty| {
            parser(move |input| {
                let (value, consumed) =
                    env_parser((ctx, &ty), inline_value_infer).parse_stream(input)?;
                Ok(((value, ty.clone()), consumed))
            })
        })
        .parse_stream(input)
}

/// Parse an inline value with optional type context.
fn inline_value<I>(
    (ctx, ty): (&NameTable, Option<&Type>),
    input: I,
) -> ParseResult<(ValueRef, Type), I>
where
    I: Stream<Item = char>,
{
    use num::Zero;

    // Parser for numeric constants (including an optional leading '-').
    let const_int = || {
        (
            optional(token('-')),
            many1(digit()).map(|s: String| BigInt::parse_bytes(s.as_bytes(), 10).unwrap()),
        )
            .map(|(sign, value)| match sign {
                Some(_) => -value,
                None => value,
            })
    };

    // Parser for SI prefices.
    let si_prefix = optional(choice!(
        token('a').map(|_| -18),
        token('f').map(|_| -15),
        token('p').map(|_| -12),
        token('n').map(|_| -9),
        token('u').map(|_| -6),
        token('m').map(|_| -3),
        // 0
        token('k').map(|_| 3),
        token('M').map(|_| 6),
        token('G').map(|_| 9),
        token('T').map(|_| 12),
        token('P').map(|_| 15),
        token('E').map(|_| 18)
    ))
    .map(|v: Option<isize>| v.unwrap_or(0));

    // Parser for time constants.
    let const_time = (
        (
            optional(token('-')),
            many1(digit()),
            optional(token('.').with(many1(digit()))),
            si_prefix.skip(token('s')),
        )
            .map(
                |(sign, int, frac, scale): (_, String, Option<String>, isize)| {
                    // Concatenate the integer and fraction part into one number.
                    let mut numer = int;
                    if let Some(ref frac) = frac {
                        numer.push_str(frac);
                    }
                    let mut denom = String::from("1");

                    // Calculate the exponent the numerator needs to be multiplied with
                    // to arrive at the correct value. If it is negative, i.e. the order
                    // of magnitude needs to be reduced, append that amount of zeros to
                    // the denominator. If it is positive, i.e. the order of magnitude
                    // needs to be increased, append that amount of zeros to the
                    // numerator.
                    let zeros = scale - frac.map(|s| s.len() as isize).unwrap_or(0);
                    if zeros < 0 {
                        denom.extend(std::iter::repeat('0').take(-zeros as usize))
                    } else if zeros > 0 {
                        numer.extend(std::iter::repeat('0').take(zeros as usize))
                    }

                    // Convert the values to BigInt and combine them into a rational
                    // number.
                    let numer = BigInt::parse_bytes(numer.as_bytes(), 10).unwrap();
                    let denom = BigInt::parse_bytes(denom.as_bytes(), 10).unwrap();
                    let v = BigRational::new(numer, denom);
                    match sign {
                        Some(_) => -v,
                        None => v,
                    }
                },
            ),
        optional(r#try(
            parser(whitespace)
                .with(many1(digit()))
                .map(|s: String| s.parse().expect("invalid delta value"))
                .skip(token('d')),
        ))
        .map(|v| v.unwrap_or(0)),
        optional(r#try(
            parser(whitespace)
                .with(many1(digit()))
                .map(|s: String| s.parse().expect("invalid epsilon value"))
                .skip(token('e')),
        ))
        .map(|v| v.unwrap_or(0)),
    );

    // Parser for array aggregates.
    let array_aggregate = (
        token('['),
        optional(r#try(
            many1(digit())
                .skip(token('x'))
                .map(|d: String| d.parse().unwrap()),
        )),
        parser(ty_parser).then(|ty| {
            parser(move |input| {
                let (values, consumed): (Vec<_>, _) = sep_by(
                    parser(whitespace).with(env_parser((ctx, &ty), inline_value_infer)),
                    lex(token(',')),
                )
                .parse_stream(input)?;
                Ok(((ty.clone(), values), consumed))
            })
        }),
        token(']'),
    )
        .map(|(_, length, (ty, values), _)| {
            let ty = array_ty(length.unwrap_or(values.len()), ty);
            (
                Aggregate::new(ArrayAggregate::new(ty.clone(), values).into()).into(),
                ty,
            )
        });

    // Parser for struct aggregates.
    let struct_aggregate = (
        token('{'),
        sep_by(env_parser((ctx, None), inline_value), lex(token(','))),
        token('}'),
    )
        .map(|(_, fields, _)| {
            let fields: Vec<_> = fields;
            let mut field_values = vec![];
            let mut field_types = vec![];
            for (v, t) in fields {
                field_values.push(v);
                field_types.push(t);
            }
            let ty = struct_ty(field_types);
            (
                Aggregate::new(StructAggregate::new(ty.clone(), field_values).into()).into(),
                ty,
            )
        });

    choice!(
        r#try((
            optional(parser(ty_parser).skip(parser(whitespace))),
            parser(name)
        ))
        .map(|(_ty, (g, s))| ctx.lookup(&NameKey(g, s))),
        r#try(array_aggregate),
        r#try(struct_aggregate),
        r#try(const_time).map(|(time, delta, epsilon)| (
            konst::const_time(time, delta, epsilon).into(),
            time_ty()
        )),
        r#try((
            optional(parser(ty_parser).skip(parser(whitespace))),
            const_int()
        ))
        .map(|(local_ty, value)| {
            let k = konst::const_int(
                local_ty
                    .as_ref()
                    .or(ty)
                    .expect("cannot infer type of integer")
                    .unwrap_int(),
                value,
            );
            let ty = k.ty();
            (k.into(), ty)
        })
    )
    .parse_stream(input)
}

/// Parse an inline named value, which does not require type inference.
fn inline_named_value<I>(ctx: &NameTable, input: I) -> ParseResult<(ValueRef, Type), I>
where
    I: Stream<Item = char>,
{
    parser(name)
        .map(|(g, s)| ctx.lookup(&NameKey(g, s)))
        .parse_stream(input)
}

/// Parse an inline block reference. This is special because it creates the
/// block if it does not yet exist, allowing for blocks to be referenced before
/// they are declared.
fn inline_label<I>(ctx: &NameTable, input: I) -> ParseResult<BlockRef, I>
where
    I: Stream<Item = char>,
{
    parser(local_name)
        .map(|s| ctx.use_block(s))
        .parse_stream(input)
}

/// Parse a list of arguments in parenthesis.
fn arguments<I>(input: I) -> ParseResult<Vec<(Type, Option<String>)>, I>
where
    I: Stream<Item = char>,
{
    between(
        lex(token('(')),
        token(')'),
        sep_by(
            parser(ty_parser)
                .skip(parser(whitespace))
                .and(optional(parser(local_name))),
            lex(token(',')),
        ),
    )
    .parse_stream(input)
}

/// Parse a function.
fn function<I>(ctx: &NameTable, input: I) -> ParseResult<Function, I>
where
    I: Stream<Item = char>,
{
    // Parse the function header.
    let (((global, name), args, return_ty), consumed) = lex(string("func"))
        .with((
            lex(parser(name)),
            lex(parser(arguments)),
            lex(parser(ty_parser)),
        ))
        .parse_stream(input)?;

    // Construct the function type.
    let mut arg_tys = Vec::new();
    let mut arg_names = Vec::new();
    for (ty, name) in args {
        arg_tys.push(ty);
        arg_names.push(name);
    }
    let func_ty = func_ty(arg_tys, return_ty);

    // Construct the function and assign names to the arguments.
    let mut func = Function::new(name.clone(), func_ty.clone());
    ctx.insert(NameKey(global, name), func.as_ref().into(), func_ty);
    let ctx = &NameTable::new(Some(ctx));
    for (name, arg) in arg_names.into_iter().zip(func.args_mut().into_iter()) {
        if let Some(name) = name {
            ctx.insert(NameKey(false, name.clone()), arg.as_ref().into(), arg.ty());
            if let Some(name) = untemp_name(name) {
                arg.set_name(name);
            }
        }
    }

    // Parse the function body.
    let (_, consumed) = consumed.combine(|input| parse_body(ctx, input, func.body_mut()))?;

    Ok((func, consumed))
}

/// Parse a process.
fn process<I>(ctx: &NameTable, input: I) -> ParseResult<Process, I>
where
    I: Stream<Item = char>,
{
    // Parse the process header.
    let ((global, name, proc_ty, in_names, out_names), consumed) = parse_header(input, "proc")?;

    // Construct the process and assign names to the arguments.
    let mut prok = Process::new(name.clone(), proc_ty.clone());
    ctx.insert(NameKey(global, name), prok.as_ref().into(), proc_ty);
    let ctx = &NameTable::new(Some(ctx));
    let assign_names = |names: Vec<Option<String>>, args: &mut [Argument]| {
        for (name, arg) in names.into_iter().zip(args.into_iter()) {
            if let Some(name) = name {
                ctx.insert(NameKey(false, name.clone()), arg.as_ref().into(), arg.ty());
                if let Some(name) = untemp_name(name) {
                    arg.set_name(name);
                }
            }
        }
    };
    assign_names(in_names, prok.inputs_mut());
    assign_names(out_names, prok.outputs_mut());

    // Parse the process body.
    let (_, consumed) = consumed.combine(|input| parse_body(ctx, input, prok.body_mut()))?;

    Ok((prok, consumed))
}

/// Parse an entity.
fn entity<I>(ctx: &NameTable, input: I) -> ParseResult<Entity, I>
where
    I: Stream<Item = char>,
{
    // Parse the entity header.
    let ((global, name, entity_ty, in_names, out_names), consumed) = parse_header(input, "entity")?;

    // Construct the entity and assign names to the arguments.
    let mut entity = Entity::new(name.clone(), entity_ty.clone());
    ctx.insert(NameKey(global, name), entity.as_ref().into(), entity_ty);
    let ctx = &NameTable::new(Some(ctx));
    let assign_names = |names: Vec<Option<String>>, args: &mut [Argument]| {
        for (name, arg) in names.into_iter().zip(args.into_iter()) {
            if let Some(name) = name {
                ctx.insert(NameKey(false, name.clone()), arg.as_ref().into(), arg.ty());
                if let Some(name) = untemp_name(name) {
                    arg.set_name(name);
                }
            }
        }
    };
    assign_names(in_names, entity.inputs_mut());
    assign_names(out_names, entity.outputs_mut());

    // Parse the entity body.
    let (insts, consumed) = consumed.combine(|input| {
        between(
            token('{').skip(parser(eol)),
            token('}').skip(parser(eol)),
            env_parser(ctx, insts),
        )
        .parse_stream(input)
    })?;
    for inst in insts {
        entity.add_inst(inst, InstPosition::End);
    }

    Ok((entity, consumed))
}

/// Parse the body of a function or process.
fn parse_body<I>(ctx: &NameTable, input: I, body: &mut SeqBody) -> ParseResult<(), I>
where
    I: Stream<Item = char>,
{
    let (blocks, consumed) = between(
        token('{').skip(parser(eol)),
        token('}').skip(parser(eol)),
        env_parser(ctx, blocks),
    )
    .parse_stream(input)?;

    for (block, insts) in blocks {
        let bb = body.add_block(block, BlockPosition::End);
        for inst in insts {
            body.add_inst(inst, InstPosition::BlockEnd(bb));
        }
    }

    Ok(((), consumed))
}

/// Parse the header of a process or entity.
fn parse_header<I>(
    input: I,
    keyword: &'static str,
) -> ParseResult<(bool, String, Type, Vec<Option<String>>, Vec<Option<String>>), I>
where
    I: Stream<Item = char>,
{
    // Parse the header.
    let (((global, name), ins, outs), consumed) = lex(string(keyword))
        .with((
            lex(parser(name)),
            lex(parser(arguments)),
            lex(parser(arguments)),
        ))
        .parse_stream(input)?;

    // Construct the type.
    let split = |args| {
        let mut arg_tys = Vec::new();
        let mut arg_names = Vec::new();
        for (ty, name) in args {
            arg_tys.push(ty);
            arg_names.push(name);
        }
        (arg_tys, arg_names)
    };
    let (in_tys, in_names) = split(ins);
    let (out_tys, out_names) = split(outs);
    let unit_ty = entity_ty(in_tys, out_tys);

    Ok(((global, name, unit_ty, in_names, out_names), consumed))
}

/// Parse a module.
fn module<I>(input: I) -> ParseResult<Module, I>
where
    I: Stream<Item = char>,
{
    let mut module = Module::new();
    let tbl = NameTable::new(None);

    enum Thing {
        Function(Function),
        Process(Process),
        Entity(Entity),
    }

    let thing = choice!(
        env_parser(&tbl, function).map(|f| Thing::Function(f)),
        env_parser(&tbl, process).map(|p| Thing::Process(p)),
        env_parser(&tbl, entity).map(|e| Thing::Entity(e))
    );

    (parser(leading_whitespace), many::<Vec<_>, _>(thing), eof())
        .parse_stream(input)
        .map(|((_, things, _), r)| {
            for thing in things {
                match thing {
                    Thing::Function(f) => {
                        module.add_function(f);
                    }
                    Thing::Process(p) => {
                        module.add_process(p);
                    }
                    Thing::Entity(e) => {
                        module.add_entity(e);
                    }
                }
            }
            (module, r)
        })
}

/// Make a name `None` if it consists only of digits.
///
/// This is useful for filtering out temporary names read from the input.
fn untemp_name(input: impl AsRef<str>) -> Option<String> {
    match input.as_ref().chars().all(|c| c.is_digit(10)) {
        false => Some(input.as_ref().into()),
        true => None,
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct NameKey(bool, String);

struct NameTable<'tp> {
    parent: Option<&'tp NameTable<'tp>>,
    values: Rc<RefCell<HashMap<NameKey, (ValueRef, Type)>>>,
    blocks: Rc<RefCell<HashMap<String, Block>>>,
}

impl<'tp> NameTable<'tp> {
    /// Create a new name table with an optional parent.
    pub fn new(parent: Option<&'tp NameTable<'tp>>) -> NameTable<'tp> {
        NameTable {
            parent: parent,
            values: Rc::new(RefCell::new(HashMap::new())),
            blocks: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    /// Insert a name into the table.
    pub fn insert(&self, key: NameKey, value: ValueRef, ty: Type) {
        let mut map = self.values.borrow_mut();
        if map.insert(key, (value, ty)).is_some() {
            panic!("name redefined");
        }
    }

    /// Lookup a name in the table.
    pub fn lookup(&self, key: &NameKey) -> (ValueRef, Type) {
        if let Some(v) = self.values.borrow().get(key) {
            return v.clone();
        }
        if let Some(p) = self.parent {
            return p.lookup(key);
        }
        panic!(
            "name {}{} has not been declared",
            if key.0 { "@" } else { "%" },
            key.1
        );
    }

    /// Lookup a block in the table. This will create the block if it does not
    /// exist, allowing blocks to be used before they are declared.
    pub fn use_block(&self, name: String) -> BlockRef {
        // Return any value with this name that is already listed.
        let k = NameKey(false, name);
        match self.values.borrow().get(&k) {
            Some(&(ValueRef::Block(r), _)) => return r,
            Some(_) => panic!("%{} does not refer to a block", k.1),
            None => (),
        }
        let name = k.1;

        // Otherwise create a new block, add it to the map of values and blocks,
        // and return a reference to it.
        let blk = Block::new(untemp_name(&name));
        let r = blk.as_ref();
        if self.blocks.borrow_mut().insert(name.clone(), blk).is_some() {
            panic!("block redefined");
        }
        if self
            .values
            .borrow_mut()
            .insert(NameKey(false, name), (r.into(), void_ty()))
            .is_some()
        {
            panic!("block redefined");
        }
        r
    }

    /// Create a new block with the given name, or take ownership of the block
    /// if it was previously allocated by `use_block`.
    pub fn declare_block(&self, name: String) -> Block {
        // If the block has already been declared, return it.
        if let Some(block) = self.blocks.borrow_mut().remove(&name) {
            return block;
        }

        // Otherwise create one, add it to the name table, and return it.
        let blk = Block::new(untemp_name(&name));
        let r: ValueRef = blk.as_ref().into();
        if self
            .values
            .borrow_mut()
            .insert(NameKey(false, name), (r.clone(), void_ty()))
            .is_some()
        {
            panic!("block redefined");
        }
        blk
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::const_int;
    use combine::{env_parser, parser, Parser, State};

    fn parse_inline_value_infer(input: &str) -> ValueRef {
        let ctx = NameTable::new(None);
        let (value, rest) = env_parser((&ctx, &void_ty()), inline_value_infer)
            .parse(State::new(input))
            .unwrap();
        if !rest.input.is_empty() {
            panic!("not all of `{}` consumed, `{}` left", input, rest.input);
        }
        value
    }

    #[test]
    fn const_time() {
        let parse = |input| {
            parse_inline_value_infer(input)
                .into_const()
                .as_time()
                .clone()
        };
        assert_eq!(
            parse("1ns"),
            konst::ConstTime::new((1.into(), (1000000000 as isize).into()).into(), 0, 0)
        );
        assert_eq!(
            parse("-2ns"),
            konst::ConstTime::new(((-2).into(), (1000000000 as isize).into()).into(), 0, 0)
        );
        assert_eq!(
            parse("3.45ns"),
            konst::ConstTime::new((345.into(), (100000000000 as isize).into()).into(), 0, 0)
        );
        assert_eq!(
            parse("-4.56ns"),
            konst::ConstTime::new(((-456).into(), (100000000000 as isize).into()).into(), 0, 0)
        );
        assert_eq!(parse("0s 1d"), konst::ConstTime::new(num::zero(), 1, 0));
        assert_eq!(parse("0s 1e"), konst::ConstTime::new(num::zero(), 0, 1));
        assert_eq!(
            parse("0s 42d 9001e"),
            konst::ConstTime::new(num::zero(), 42, 9001)
        );
    }

    #[test]
    fn array_aggregate() {
        let parse = |input| parse_inline_value_infer(input).unwrap_aggregate().clone();
        assert_eq!(
            parse("[i32]"),
            Aggregate::new(ArrayAggregate::new(array_ty(0, int_ty(32)), vec![]).into())
        );
        assert_eq!(
            parse("[i32 42]"),
            Aggregate::new(
                ArrayAggregate::new(
                    array_ty(1, int_ty(32)),
                    vec![const_int(32, BigInt::from(42)).into()]
                )
                .into()
            )
        );
        assert_eq!(
            parse("[i32 42, 9001]"),
            Aggregate::new(
                ArrayAggregate::new(
                    array_ty(2, int_ty(32)),
                    vec![
                        const_int(32, BigInt::from(42)).into(),
                        const_int(32, BigInt::from(9001)).into()
                    ]
                )
                .into()
            )
        );
        assert_eq!(
            parse("[i32 42, i32 9001]"),
            Aggregate::new(
                ArrayAggregate::new(
                    array_ty(2, int_ty(32)),
                    vec![
                        const_int(32, BigInt::from(42)).into(),
                        const_int(32, BigInt::from(9001)).into()
                    ]
                )
                .into()
            )
        );
    }

    #[test]
    fn struct_aggregate() {
        let parse = |input| parse_inline_value_infer(input).unwrap_aggregate().clone();
        assert_eq!(
            parse("{}"),
            Aggregate::new(StructAggregate::new(struct_ty(vec![]), vec![]).into())
        );
        assert_eq!(
            parse("{i32 42}"),
            Aggregate::new(
                StructAggregate::new(
                    struct_ty(vec![int_ty(32)]),
                    vec![const_int(32, BigInt::from(42)).into()]
                )
                .into()
            )
        );
        assert_eq!(
            parse("{i32 42, i64 9001}"),
            Aggregate::new(
                StructAggregate::new(
                    struct_ty(vec![int_ty(32), int_ty(64)]),
                    vec![
                        const_int(32, BigInt::from(42)).into(),
                        const_int(64, BigInt::from(9001)).into()
                    ]
                )
                .into()
            )
        );
    }

    #[test]
    fn types() {
        let parse = |input| parser(super::ty_parser).parse(State::new(input)).unwrap().0;
        assert_eq!(parse("void"), void_ty());
        assert_eq!(parse("time"), time_ty());
        assert_eq!(parse("i8"), int_ty(8));
        assert_eq!(parse("n42"), enum_ty(42));
        assert_eq!(parse("i32*"), pointer_ty(int_ty(32)));
        assert_eq!(parse("i32$"), signal_ty(int_ty(32)));
        // assert_eq!(parse("<42 x i8>"), array_ty(42, int_ty(8)));
        // assert_eq!(parse("{void, time, i8}"), struct_ty(vec![void_ty(), time_ty(), int_ty(8)]));
        // assert_eq!(parse("(i8, time) void}"), func_ty(vec![int_ty(8), time_ty()], void_ty()));
        // assert_eq!(parse("(i8$; i42$)"), entity_ty(vec![signal_ty(int_ty(8))], vec![signal_ty(int_ty(42))]));
    }

    #[test]
    fn compare_ops() {
        use crate::CompareOp;
        let parse = |input| match env_parser(&NameTable::new(None), super::compare_inst)
            .parse(State::new(input))
            .unwrap()
            .0
        {
            crate::CompareInst(op, ..) => op,
            _ => panic!("did not yield a compare inst"),
        };
        assert_eq!(parse("cmp eq i1 0 0"), CompareOp::Eq);
        assert_eq!(parse("cmp neq i1 0 0"), CompareOp::Neq);
        assert_eq!(parse("cmp slt i1 0 0"), CompareOp::Slt);
        assert_eq!(parse("cmp sgt i1 0 0"), CompareOp::Sgt);
        assert_eq!(parse("cmp sle i1 0 0"), CompareOp::Sle);
        assert_eq!(parse("cmp sge i1 0 0"), CompareOp::Sge);
        assert_eq!(parse("cmp ult i1 0 0"), CompareOp::Ult);
        assert_eq!(parse("cmp ugt i1 0 0"), CompareOp::Ugt);
        assert_eq!(parse("cmp ule i1 0 0"), CompareOp::Ule);
        assert_eq!(parse("cmp uge i1 0 0"), CompareOp::Uge);
    }

    #[test]
    fn memory_inst() {
        let parse = |input| {
            env_parser(&NameTable::new(None), super::insts)
                .parse(State::new(input))
                .unwrap()
                .0
        };
        let insts = parse(indoc::indoc! {"
            %0 = var i32
            load i32 %0
            store i32 0 %0
        "});
        assert_eq!(insts[0].kind(), &VariableInst(int_ty(32)));
        assert_eq!(
            insts[1].kind(),
            &LoadInst(int_ty(32), insts[0].as_ref().into())
        );
        assert_eq!(
            insts[2].kind(),
            &StoreInst(
                int_ty(32),
                const_int(32, 0.into()).into(),
                insts[0].as_ref().into()
            )
        );
    }
}
