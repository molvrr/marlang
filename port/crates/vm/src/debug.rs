use crate::bytecode::{Chunk, ConstantValue, Instruction, Op};

/// Disassemble a chunk into a human-readable string.
pub fn disassemble(chunk: &Chunk, name: Option<&str>) -> String {
    let mut output = format!("=== {} ===\n", name.unwrap_or("<script>"));

    for i in 0..chunk.code.len() {
        output += &format_instruction(chunk, i);
    }

    // Disassemble nested prototypes
    for constant in &chunk.constants {
        if let ConstantValue::Prototype(proto) = constant {
            output += "\n";
            output += &disassemble(&proto.chunk, Some(&proto.name));
        }
    }

    output
}

/// Format a single instruction for display.
pub fn format_instruction(chunk: &Chunk, offset: usize) -> String {
    let inst = &chunk.code[offset];
    let line = inst.line;
    let op_name = inst.op.name();

    let mut base = format!("{:4} | {:4} | {:<16}", offset, line, op_name);

    if has_operand(inst) {
        base += &format!(" {:4}", inst.operand);

        if let Some(annotation) = get_annotation(chunk, inst) {
            base += &format!("    ; {}", annotation);
        }

        if has_operand2(inst) {
            base += &format!(" {:4}", inst.operand2);
        }
    }

    base + "\n"
}

/// Get an annotation for an instruction that references a constant.
pub fn get_annotation(chunk: &Chunk, inst: &Instruction) -> Option<String> {
    match inst.op {
        Op::PushConst
        | Op::PushGlobal
        | Op::SetGlobal
        | Op::MatchTag
        | Op::Construct
        | Op::Perform => {
            let constant = chunk.constants.get(inst.operand as usize)?;
            Some(format_constant(constant))
        }
        Op::Closure => {
            let constant = chunk.constants.get(inst.operand as usize)?;
            if let ConstantValue::Prototype(proto) = constant {
                let variadic_marker = if proto.is_variadic { "+" } else { "" };
                Some(format!(
                    "<fn {}/{}{}>",
                    proto.name, proto.arity, variadic_marker
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn format_constant(constant: &ConstantValue) -> String {
    match constant {
        ConstantValue::Number(n) => n.to_string(),
        ConstantValue::String(s) => {
            if s.starts_with('\0') {
                "<special>".to_string()
            } else if s.len() > 30 {
                format!("\"{}...\"", &s[..30])
            } else {
                format!("\"{}\"", s)
            }
        }
        ConstantValue::Boolean(b) => {
            if *b {
                "#t".to_string()
            } else {
                "#f".to_string()
            }
        }
        ConstantValue::Nil => "nil".to_string(),
        ConstantValue::Prototype(proto) => {
            let variadic_marker = if proto.is_variadic { "+" } else { "" };
            format!("<fn {}/{}{}>", proto.name, proto.arity, variadic_marker)
        }
        ConstantValue::FfiSigs(sigs) => {
            format!("<ffi-sigs({})>", sigs.len())
        }
        ConstantValue::FfiBound { lib_name, fn_name } => {
            format!("<ffi-bound {}/{}>", lib_name, fn_name)
        }
    }
}

/// Check if an instruction uses its operand field.
fn has_operand(inst: &Instruction) -> bool {
    matches!(
        inst.op,
        Op::PushConst
            | Op::PushLocal
            | Op::SetLocal
            | Op::PushUpvalue
            | Op::SetUpvalue
            | Op::PushGlobal
            | Op::SetGlobal
            | Op::Jump
            | Op::JumpIfFalse
            | Op::Call
            | Op::TailCall
            | Op::Closure
            | Op::Construct
            | Op::GetField
            | Op::MatchTag
            | Op::PushHandler
            | Op::Perform
    )
}

/// Check if an instruction uses its operand2 field.
fn has_operand2(inst: &Instruction) -> bool {
    matches!(inst.op, Op::Construct | Op::MatchTag | Op::Perform)
}
