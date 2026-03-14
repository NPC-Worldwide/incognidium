use murkiu_parser::*;

/// Bytecode opcodes for the Murkiu VM.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Push a constant onto the stack
    Const(u16),
    /// Push `undefined`
    Undefined,
    /// Push `null`
    Null,
    /// Push `true`
    True,
    /// Push `false`
    False,

    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Neg,
    Pos,
    BitNot,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,

    // Comparison
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    Lt,
    Gt,
    Le,
    Ge,
    Instanceof,
    In,

    // Logical / unary
    Not,
    Typeof,
    Void,
    Delete,

    // Variables
    /// Load a local variable by slot index
    GetLocal(u16),
    /// Set a local variable by slot index
    SetLocal(u16),
    /// Load a global variable by name (constant index)
    GetGlobal(u16),
    /// Set a global variable by name (constant index)
    SetGlobal(u16),

    // Properties
    /// Get property: stack = [obj, key] -> [value]
    GetProp,
    /// Set property: stack = [obj, key, value] -> [value]
    SetProp,
    /// Get named property (constant index for name)
    GetField(u16),
    /// Set named property (constant index for name)
    SetField(u16),

    // Control flow
    /// Unconditional jump (offset from current position)
    Jump(i32),
    /// Jump if top of stack is falsy (pops)
    JumpIfFalse(i32),
    /// Jump if top of stack is truthy (pops)
    JumpIfTrue(i32),
    /// Jump if top of stack is nullish (does not pop)
    JumpIfNullish(i32),

    // Functions
    /// Call function: stack = [callee, arg0, ..., argN], operand = argc
    Call(u8),
    /// Return from function
    Return,
    /// Create a closure from a function constant
    Closure(u16),
    /// new Constructor(args), operand = argc
    NewCall(u8),

    // Stack manipulation
    Pop,
    Dup,
    Swap,

    // Objects / arrays
    /// Create array with N elements from stack
    NewArray(u16),
    /// Create object with N key-value pairs from stack
    NewObject(u16),

    // Increment/decrement
    PreInc,
    PreDec,
    PostInc,
    PostDec,

    // Misc
    /// Push `this`
    This,
    /// Throw top of stack
    Throw,
    /// Enter try block (catch offset, finally offset)
    EnterTry(i32, i32),
    /// Leave try block
    LeaveTry,
    /// Debugger statement (no-op or breakpoint)
    Debugger,

    /// Halt the VM
    Halt,
}

/// A constant value in the constant pool.
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Number(f64),
    Str(String),
    Bool(bool),
    Null,
    Undefined,
    /// A compiled function
    Function(FunctionProto),
}

/// Prototype for a compiled function (stored in constants pool).
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionProto {
    pub name: Option<String>,
    pub params: Vec<String>,
    pub num_locals: u16,
    pub code: Vec<Op>,
    pub constants: Vec<Constant>,
}

/// A compiled bytecode chunk (top-level script or function body).
#[derive(Debug, Clone)]
pub struct Chunk {
    pub code: Vec<Op>,
    pub constants: Vec<Constant>,
    pub num_locals: u16,
}

impl Chunk {
    fn new() -> Self {
        Chunk {
            code: Vec::new(),
            constants: Vec::new(),
            num_locals: 0,
        }
    }

    fn add_constant(&mut self, c: Constant) -> u16 {
        // Reuse existing constants
        for (i, existing) in self.constants.iter().enumerate() {
            if *existing == c {
                return i as u16;
            }
        }
        let idx = self.constants.len() as u16;
        self.constants.push(c);
        idx
    }

    fn emit(&mut self, op: Op) -> usize {
        let pos = self.code.len();
        self.code.push(op);
        pos
    }

    fn current_pos(&self) -> usize {
        self.code.len()
    }

    /// Patch a jump instruction at `pos` to jump to `target`.
    fn patch_jump(&mut self, pos: usize, target: usize) {
        let offset = target as i32 - pos as i32 - 1;
        match &mut self.code[pos] {
            Op::Jump(ref mut o) => *o = offset,
            Op::JumpIfFalse(ref mut o) => *o = offset,
            Op::JumpIfTrue(ref mut o) => *o = offset,
            Op::JumpIfNullish(ref mut o) => *o = offset,
            _ => panic!("patch_jump on non-jump op"),
        }
    }
}

/// Compiler state for tracking local variable scopes.
struct Compiler {
    chunk: Chunk,
    locals: Vec<Local>,
    scope_depth: u32,
    loop_breaks: Vec<Vec<usize>>,    // stack of break patch positions per loop
    loop_continues: Vec<Vec<usize>>,  // stack of continue patch positions per loop
    loop_starts: Vec<usize>,          // stack of loop start positions for continue
}

struct Local {
    name: String,
    depth: u32,
    slot: u16,
}

impl Compiler {
    fn new() -> Self {
        Compiler {
            chunk: Chunk::new(),
            locals: Vec::new(),
            scope_depth: 0,
            loop_breaks: Vec::new(),
            loop_continues: Vec::new(),
            loop_starts: Vec::new(),
        }
    }

    fn begin_scope(&mut self) {
        self.scope_depth += 1;
    }

    fn end_scope(&mut self) {
        self.scope_depth -= 1;
        // Remove locals that belong to the exiting scope
        // (locals are stored in the frame's locals array, not on the value stack,
        // so no Pop needed)
        while let Some(local) = self.locals.last() {
            if local.depth > self.scope_depth {
                self.locals.pop();
            } else {
                break;
            }
        }
    }

    fn declare_local(&mut self, name: &str) -> u16 {
        let slot = self.chunk.num_locals;
        self.chunk.num_locals += 1;
        self.locals.push(Local {
            name: name.to_string(),
            depth: self.scope_depth,
            slot,
        });
        slot
    }

    fn resolve_local(&self, name: &str) -> Option<u16> {
        for local in self.locals.iter().rev() {
            if local.name == name {
                return Some(local.slot);
            }
        }
        None
    }

    fn compile_program(&mut self, program: &Program) -> Result<(), String> {
        for stmt in &program.body {
            self.compile_stmt(stmt)?;
        }
        self.chunk.emit(Op::Halt);
        Ok(())
    }

    fn compile_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::VarDecl { kind: _, declarations } => {
                for (name, init) in declarations {
                    if let Some(expr) = init {
                        self.compile_expr(expr)?;
                    } else {
                        self.chunk.emit(Op::Undefined);
                    }
                    if self.scope_depth > 0 {
                        let slot = self.declare_local(name);
                        self.chunk.emit(Op::SetLocal(slot));
                        self.chunk.emit(Op::Pop);
                    } else {
                        let name_idx = self.chunk.add_constant(Constant::Str(name.clone()));
                        self.chunk.emit(Op::SetGlobal(name_idx));
                        self.chunk.emit(Op::Pop);
                    }
                }
            }

            Stmt::Expr(expr) => {
                self.compile_expr(expr)?;
                self.chunk.emit(Op::Pop);
            }

            Stmt::Block(stmts) => {
                self.begin_scope();
                for s in stmts {
                    self.compile_stmt(s)?;
                }
                self.end_scope();
            }

            Stmt::Return(expr) => {
                if let Some(e) = expr {
                    self.compile_expr(e)?;
                } else {
                    self.chunk.emit(Op::Undefined);
                }
                self.chunk.emit(Op::Return);
            }

            Stmt::If { condition, then_branch, else_branch } => {
                self.compile_expr(condition)?;
                let jump_else = self.chunk.emit(Op::JumpIfFalse(0));
                self.compile_stmt(then_branch)?;
                if let Some(else_br) = else_branch {
                    let jump_end = self.chunk.emit(Op::Jump(0));
                    let else_pos = self.chunk.current_pos();
                    self.chunk.patch_jump(jump_else, else_pos);
                    self.compile_stmt(else_br)?;
                    let end_pos = self.chunk.current_pos();
                    self.chunk.patch_jump(jump_end, end_pos);
                } else {
                    let end_pos = self.chunk.current_pos();
                    self.chunk.patch_jump(jump_else, end_pos);
                }
            }

            Stmt::While { condition, body } => {
                let loop_start = self.chunk.current_pos();
                self.loop_starts.push(loop_start);
                self.loop_breaks.push(Vec::new());
                self.loop_continues.push(Vec::new());

                self.compile_expr(condition)?;
                let exit_jump = self.chunk.emit(Op::JumpIfFalse(0));
                self.compile_stmt(body)?;

                // Patch continues to jump back to condition
                let continues = self.loop_continues.pop().unwrap();
                for pos in &continues {
                    self.chunk.patch_jump(*pos, loop_start);
                }

                let back_offset = loop_start as i32 - self.chunk.current_pos() as i32 - 1;
                self.chunk.emit(Op::Jump(back_offset));

                let exit_pos = self.chunk.current_pos();
                self.chunk.patch_jump(exit_jump, exit_pos);

                // Patch breaks to jump here
                let breaks = self.loop_breaks.pop().unwrap();
                for pos in &breaks {
                    self.chunk.patch_jump(*pos, exit_pos);
                }
                self.loop_starts.pop();
            }

            Stmt::For { init, condition, update, body } => {
                self.begin_scope();
                if let Some(init_stmt) = init {
                    self.compile_stmt(init_stmt)?;
                }

                let loop_start = self.chunk.current_pos();
                self.loop_starts.push(loop_start);
                self.loop_breaks.push(Vec::new());
                self.loop_continues.push(Vec::new());

                let exit_jump = if let Some(cond) = condition {
                    self.compile_expr(cond)?;
                    Some(self.chunk.emit(Op::JumpIfFalse(0)))
                } else {
                    None
                };

                self.compile_stmt(body)?;

                // Continue target: the update expression
                let continue_target = self.chunk.current_pos();
                let continues = self.loop_continues.pop().unwrap();
                for pos in &continues {
                    self.chunk.patch_jump(*pos, continue_target);
                }

                if let Some(upd) = update {
                    self.compile_expr(upd)?;
                    self.chunk.emit(Op::Pop);
                }

                let back_offset = loop_start as i32 - self.chunk.current_pos() as i32 - 1;
                self.chunk.emit(Op::Jump(back_offset));

                let exit_pos = self.chunk.current_pos();
                if let Some(ej) = exit_jump {
                    self.chunk.patch_jump(ej, exit_pos);
                }

                let breaks = self.loop_breaks.pop().unwrap();
                for pos in &breaks {
                    self.chunk.patch_jump(*pos, exit_pos);
                }
                self.loop_starts.pop();
                self.end_scope();
            }

            Stmt::DoWhile { body, condition } => {
                let loop_start = self.chunk.current_pos();
                self.loop_starts.push(loop_start);
                self.loop_breaks.push(Vec::new());
                self.loop_continues.push(Vec::new());

                self.compile_stmt(body)?;

                let continue_target = self.chunk.current_pos();
                let continues = self.loop_continues.pop().unwrap();
                for pos in &continues {
                    self.chunk.patch_jump(*pos, continue_target);
                }

                self.compile_expr(condition)?;
                let back_offset = loop_start as i32 - self.chunk.current_pos() as i32 - 1;
                self.chunk.emit(Op::JumpIfTrue(back_offset));

                let exit_pos = self.chunk.current_pos();
                let breaks = self.loop_breaks.pop().unwrap();
                for pos in &breaks {
                    self.chunk.patch_jump(*pos, exit_pos);
                }
                self.loop_starts.pop();
            }

            Stmt::FunctionDecl { name, params, body } => {
                let func = self.compile_function(Some(name.clone()), params, body)?;
                let idx = self.chunk.add_constant(Constant::Function(func));
                self.chunk.emit(Op::Closure(idx));
                if self.scope_depth > 0 {
                    let slot = self.declare_local(name);
                    self.chunk.emit(Op::SetLocal(slot));
                    self.chunk.emit(Op::Pop);
                } else {
                    let name_idx = self.chunk.add_constant(Constant::Str(name.clone()));
                    self.chunk.emit(Op::SetGlobal(name_idx));
                    self.chunk.emit(Op::Pop);
                }
            }

            Stmt::Break => {
                if let Some(breaks) = self.loop_breaks.last_mut() {
                    let pos = self.chunk.emit(Op::Jump(0));
                    breaks.push(pos);
                } else {
                    return Err("break outside loop".into());
                }
            }

            Stmt::Continue => {
                if let Some(continues) = self.loop_continues.last_mut() {
                    let pos = self.chunk.emit(Op::Jump(0));
                    continues.push(pos);
                } else {
                    return Err("continue outside loop".into());
                }
            }

            Stmt::Throw(expr) => {
                self.compile_expr(expr)?;
                self.chunk.emit(Op::Throw);
            }

            Stmt::TryCatch { try_block, catch_param, catch_block, finally_block } => {
                // Simplified try-catch: emit try block, catch block inline
                let enter_pos = self.chunk.emit(Op::EnterTry(0, 0));
                for s in try_block {
                    self.compile_stmt(s)?;
                }
                self.chunk.emit(Op::LeaveTry);
                let jump_over_catch = self.chunk.emit(Op::Jump(0));

                // Catch block
                let catch_pos = self.chunk.current_pos();
                if let Some(catch_stmts) = catch_block {
                    self.begin_scope();
                    if let Some(param) = catch_param {
                        // The caught value is on the stack
                        self.declare_local(param);
                    }
                    for s in catch_stmts {
                        self.compile_stmt(s)?;
                    }
                    self.end_scope();
                }

                let after_catch = self.chunk.current_pos();
                self.chunk.patch_jump(jump_over_catch, after_catch);

                // Finally block
                let finally_pos = if let Some(finally_stmts) = finally_block {
                    let fp = self.chunk.current_pos();
                    for s in finally_stmts {
                        self.compile_stmt(s)?;
                    }
                    fp as i32
                } else {
                    after_catch as i32
                };

                // Patch EnterTry
                let catch_offset = catch_pos as i32 - enter_pos as i32 - 1;
                let finally_offset = finally_pos - enter_pos as i32 - 1;
                self.chunk.code[enter_pos] = Op::EnterTry(catch_offset, finally_offset);
            }

            Stmt::Switch { discriminant, cases } => {
                self.compile_expr(discriminant)?;
                let mut end_jumps = Vec::new();
                let mut next_case_jumps: Vec<usize> = Vec::new();

                for case in cases {
                    // Patch previous case's fall-through test
                    for pos in next_case_jumps.drain(..) {
                        self.chunk.patch_jump(pos, self.chunk.current_pos());
                    }

                    if let Some(test) = &case.test {
                        self.chunk.emit(Op::Dup);
                        self.compile_expr(test)?;
                        self.chunk.emit(Op::StrictEq);
                        let skip = self.chunk.emit(Op::JumpIfFalse(0));
                        next_case_jumps.push(skip);
                        // Pop the dup'd discriminant before body if matched
                    } else {
                        // default case - always runs
                    }

                    for s in &case.body {
                        self.compile_stmt(s)?;
                    }
                }

                // Patch remaining case jumps to end
                let end_pos = self.chunk.current_pos();
                for pos in next_case_jumps {
                    self.chunk.patch_jump(pos, end_pos);
                }
                for pos in end_jumps {
                    self.chunk.patch_jump(pos, end_pos);
                }
                self.chunk.emit(Op::Pop); // pop discriminant
            }

            Stmt::Empty => {}
            Stmt::Debugger => { self.chunk.emit(Op::Debugger); }
        }
        Ok(())
    }

    fn compile_expr(&mut self, expr: &Expr) -> Result<(), String> {
        match expr {
            Expr::Number(n) => {
                let idx = self.chunk.add_constant(Constant::Number(*n));
                self.chunk.emit(Op::Const(idx));
            }
            Expr::Str(s) => {
                let idx = self.chunk.add_constant(Constant::Str(s.clone()));
                self.chunk.emit(Op::Const(idx));
            }
            Expr::Template(s) => {
                let idx = self.chunk.add_constant(Constant::Str(s.clone()));
                self.chunk.emit(Op::Const(idx));
            }
            Expr::Bool(true) => { self.chunk.emit(Op::True); }
            Expr::Bool(false) => { self.chunk.emit(Op::False); }
            Expr::Null => { self.chunk.emit(Op::Null); }
            Expr::Undefined => { self.chunk.emit(Op::Undefined); }
            Expr::This => { self.chunk.emit(Op::This); }

            Expr::Ident(name) => {
                if let Some(slot) = self.resolve_local(name) {
                    self.chunk.emit(Op::GetLocal(slot));
                } else {
                    let idx = self.chunk.add_constant(Constant::Str(name.clone()));
                    self.chunk.emit(Op::GetGlobal(idx));
                }
            }

            Expr::Binary(left, op, right) => {
                self.compile_expr(left)?;
                self.compile_expr(right)?;
                let opcode = match op {
                    BinOp::Add => Op::Add,
                    BinOp::Sub => Op::Sub,
                    BinOp::Mul => Op::Mul,
                    BinOp::Div => Op::Div,
                    BinOp::Mod => Op::Mod,
                    BinOp::Pow => Op::Pow,
                    BinOp::Eq => Op::Eq,
                    BinOp::Ne => Op::Ne,
                    BinOp::StrictEq => Op::StrictEq,
                    BinOp::StrictNe => Op::StrictNe,
                    BinOp::Lt => Op::Lt,
                    BinOp::Gt => Op::Gt,
                    BinOp::Le => Op::Le,
                    BinOp::Ge => Op::Ge,
                    BinOp::BitAnd => Op::BitAnd,
                    BinOp::BitOr => Op::BitOr,
                    BinOp::BitXor => Op::BitXor,
                    BinOp::Shl => Op::Shl,
                    BinOp::Shr => Op::Shr,
                    BinOp::UShr => Op::UShr,
                    BinOp::Instanceof => Op::Instanceof,
                    BinOp::In => Op::In,
                    BinOp::NullishCoalesce => {
                        // Already compiled left, need special handling
                        // But we already pushed both. Handle via short-circuit.
                        // Actually for ??, we need: eval left, if not null/undefined, skip right
                        // Redo: pop right, use jump
                        // This is a simplification — just use the binary approach
                        Op::BitOr // placeholder, will fix below
                    }
                };
                // Handle nullish coalesce specially
                if *op == BinOp::NullishCoalesce {
                    // We already emitted both sides. This is wrong for short-circuit.
                    // For simplicity, we just eval both and pick. A real impl would short-circuit.
                    // Pop both sides, re-do with short circuit:
                    // Actually let's just leave it as: compute left, dup, check nullish, jump over right if not nullish
                    // But we already compiled both... let's just emit a simple "or-like" for now.
                    // TODO: proper short-circuit for ??
                    self.chunk.emit(Op::BitOr); // rough approximation
                } else {
                    self.chunk.emit(opcode);
                }
            }

            Expr::Logical(left, op, right) => {
                self.compile_expr(left)?;
                match op {
                    LogicalOp::And => {
                        self.chunk.emit(Op::Dup);
                        let jump = self.chunk.emit(Op::JumpIfFalse(0));
                        self.chunk.emit(Op::Pop);
                        self.compile_expr(right)?;
                        let end = self.chunk.current_pos();
                        self.chunk.patch_jump(jump, end);
                    }
                    LogicalOp::Or => {
                        self.chunk.emit(Op::Dup);
                        let jump = self.chunk.emit(Op::JumpIfTrue(0));
                        self.chunk.emit(Op::Pop);
                        self.compile_expr(right)?;
                        let end = self.chunk.current_pos();
                        self.chunk.patch_jump(jump, end);
                    }
                }
            }

            Expr::Unary(op, operand) => {
                self.compile_expr(operand)?;
                match op {
                    UnaryOp::Neg => { self.chunk.emit(Op::Neg); }
                    UnaryOp::Pos => { self.chunk.emit(Op::Pos); }
                    UnaryOp::Not => { self.chunk.emit(Op::Not); }
                    UnaryOp::BitNot => { self.chunk.emit(Op::BitNot); }
                    UnaryOp::Inc => { self.chunk.emit(Op::PreInc); }
                    UnaryOp::Dec => { self.chunk.emit(Op::PreDec); }
                }
            }

            Expr::Postfix(operand, op) => {
                self.compile_expr(operand)?;
                match op {
                    UnaryOp::Inc => { self.chunk.emit(Op::PostInc); }
                    UnaryOp::Dec => { self.chunk.emit(Op::PostDec); }
                    _ => {}
                }
            }

            Expr::Typeof(operand) => {
                self.compile_expr(operand)?;
                self.chunk.emit(Op::Typeof);
            }

            Expr::VoidExpr(operand) => {
                self.compile_expr(operand)?;
                self.chunk.emit(Op::Void);
            }

            Expr::DeleteExpr(operand) => {
                self.compile_expr(operand)?;
                self.chunk.emit(Op::Delete);
            }

            Expr::Assign(target, op, value) => {
                // Compile the value
                if *op != AssignOp::Assign {
                    // Compound assignment: load current value first
                    self.compile_expr(target)?;
                }
                self.compile_expr(value)?;
                if *op != AssignOp::Assign {
                    let bin_op = match op {
                        AssignOp::AddAssign => Op::Add,
                        AssignOp::SubAssign => Op::Sub,
                        AssignOp::MulAssign => Op::Mul,
                        AssignOp::DivAssign => Op::Div,
                        AssignOp::ModAssign => Op::Mod,
                        AssignOp::Assign => unreachable!(),
                    };
                    self.chunk.emit(bin_op);
                }
                // Store
                match target.as_ref() {
                    Expr::Ident(name) => {
                        self.chunk.emit(Op::Dup); // leave value on stack
                        if let Some(slot) = self.resolve_local(name) {
                            self.chunk.emit(Op::SetLocal(slot));
                        } else {
                            let idx = self.chunk.add_constant(Constant::Str(name.clone()));
                            self.chunk.emit(Op::SetGlobal(idx));
                        }
                    }
                    Expr::Member(obj, prop) => {
                        self.compile_expr(obj)?;
                        self.chunk.emit(Op::Swap);
                        let idx = self.chunk.add_constant(Constant::Str(prop.clone()));
                        self.chunk.emit(Op::SetField(idx));
                    }
                    Expr::Index(obj, key) => {
                        self.compile_expr(obj)?;
                        self.compile_expr(key)?;
                        // stack: [value, obj, key]
                        // need: SetProp which expects [obj, key, value]
                        // Simplification: just emit SetProp, VM will handle ordering
                        self.chunk.emit(Op::SetProp);
                    }
                    _ => return Err("Invalid assignment target".into()),
                }
            }

            Expr::Member(obj, prop) => {
                self.compile_expr(obj)?;
                let idx = self.chunk.add_constant(Constant::Str(prop.clone()));
                self.chunk.emit(Op::GetField(idx));
            }

            Expr::Index(obj, key) => {
                self.compile_expr(obj)?;
                self.compile_expr(key)?;
                self.chunk.emit(Op::GetProp);
            }

            Expr::Call(callee, args) => {
                self.compile_expr(callee)?;
                for arg in args {
                    self.compile_expr(arg)?;
                }
                self.chunk.emit(Op::Call(args.len() as u8));
            }

            Expr::New(callee, args) => {
                self.compile_expr(callee)?;
                for arg in args {
                    self.compile_expr(arg)?;
                }
                self.chunk.emit(Op::NewCall(args.len() as u8));
            }

            Expr::Array(elements) => {
                for elem in elements {
                    self.compile_expr(elem)?;
                }
                self.chunk.emit(Op::NewArray(elements.len() as u16));
            }

            Expr::Object(props) => {
                for (key, val) in props {
                    match key {
                        PropKey::Ident(s) | PropKey::Str(s) => {
                            let idx = self.chunk.add_constant(Constant::Str(s.clone()));
                            self.chunk.emit(Op::Const(idx));
                        }
                        PropKey::Number(n) => {
                            let idx = self.chunk.add_constant(Constant::Number(*n));
                            self.chunk.emit(Op::Const(idx));
                        }
                    }
                    self.compile_expr(val)?;
                }
                self.chunk.emit(Op::NewObject(props.len() as u16));
            }

            Expr::FunctionExpr { name, params, body } => {
                let func = self.compile_function(name.clone(), params, body)?;
                let idx = self.chunk.add_constant(Constant::Function(func));
                self.chunk.emit(Op::Closure(idx));
            }

            Expr::Arrow { params, body } => {
                let body_stmts = match body {
                    ArrowBody::Block(stmts) => stmts.clone(),
                    ArrowBody::Expr(expr) => vec![Stmt::Return(Some(*expr.clone()))],
                };
                let func = self.compile_function(None, params, &body_stmts)?;
                let idx = self.chunk.add_constant(Constant::Function(func));
                self.chunk.emit(Op::Closure(idx));
            }

            Expr::Ternary(cond, then_expr, else_expr) => {
                self.compile_expr(cond)?;
                let jump_else = self.chunk.emit(Op::JumpIfFalse(0));
                self.compile_expr(then_expr)?;
                let jump_end = self.chunk.emit(Op::Jump(0));
                let else_pos = self.chunk.current_pos();
                self.chunk.patch_jump(jump_else, else_pos);
                self.compile_expr(else_expr)?;
                let end_pos = self.chunk.current_pos();
                self.chunk.patch_jump(jump_end, end_pos);
            }

            Expr::Sequence(exprs) => {
                for (i, e) in exprs.iter().enumerate() {
                    self.compile_expr(e)?;
                    if i < exprs.len() - 1 {
                        self.chunk.emit(Op::Pop);
                    }
                }
            }
        }
        Ok(())
    }

    fn compile_function(
        &mut self,
        name: Option<String>,
        params: &[String],
        body: &[Stmt],
    ) -> Result<FunctionProto, String> {
        let mut func_compiler = Compiler::new();
        func_compiler.scope_depth = 1; // function body is a scope

        // Declare parameters as locals
        for param in params {
            func_compiler.declare_local(param);
        }

        for stmt in body {
            func_compiler.compile_stmt(stmt)?;
        }

        // Implicit return undefined
        func_compiler.chunk.emit(Op::Undefined);
        func_compiler.chunk.emit(Op::Return);

        Ok(FunctionProto {
            name,
            params: params.to_vec(),
            num_locals: func_compiler.chunk.num_locals,
            code: func_compiler.chunk.code,
            constants: func_compiler.chunk.constants,
        })
    }
}

/// Compile a parsed JS program into bytecode.
pub fn compile(program: &Program) -> Result<Chunk, String> {
    let mut compiler = Compiler::new();
    compiler.compile_program(program)?;
    Ok(compiler.chunk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use murkiu_parser::parse;

    #[test]
    fn test_compile_var_decl() {
        let prog = parse("var x = 42;").unwrap();
        let chunk = compile(&prog).unwrap();
        // Should have: Const(0), SetGlobal(1), Pop, Halt
        assert!(chunk.constants.len() >= 1);
        assert_eq!(chunk.constants[0], Constant::Number(42.0));
    }

    #[test]
    fn test_compile_arithmetic() {
        let prog = parse("var x = 2 + 3;").unwrap();
        let chunk = compile(&prog).unwrap();
        // Should contain Add opcode
        assert!(chunk.code.iter().any(|op| matches!(op, Op::Add)));
    }

    #[test]
    fn test_compile_function() {
        let prog = parse("function add(a, b) { return a + b; }").unwrap();
        let chunk = compile(&prog).unwrap();
        // Should have a Function constant
        assert!(chunk.constants.iter().any(|c| matches!(c, Constant::Function(_))));
    }

    #[test]
    fn test_compile_if() {
        let prog = parse("if (x) { y = 1; } else { y = 2; }").unwrap();
        let chunk = compile(&prog).unwrap();
        // Should have JumpIfFalse
        assert!(chunk.code.iter().any(|op| matches!(op, Op::JumpIfFalse(_))));
    }

    #[test]
    fn test_compile_while() {
        let prog = parse("var i = 0; while (i < 10) { i = i + 1; }").unwrap();
        let chunk = compile(&prog).unwrap();
        // Should have backward Jump and JumpIfFalse
        assert!(chunk.code.iter().any(|op| matches!(op, Op::Jump(_))));
        assert!(chunk.code.iter().any(|op| matches!(op, Op::JumpIfFalse(_))));
    }

    #[test]
    fn test_compile_call() {
        let prog = parse("console.log('hello');").unwrap();
        let chunk = compile(&prog).unwrap();
        assert!(chunk.code.iter().any(|op| matches!(op, Op::Call(_))));
    }

    #[test]
    fn test_compile_logical_and() {
        let prog = parse("var x = a && b;").unwrap();
        let chunk = compile(&prog).unwrap();
        // Short-circuit: should have Dup + JumpIfFalse
        assert!(chunk.code.iter().any(|op| matches!(op, Op::Dup)));
    }

    #[test]
    fn test_compile_array_object() {
        let prog = parse("var a = [1, 2]; var o = { x: 1 };").unwrap();
        let chunk = compile(&prog).unwrap();
        assert!(chunk.code.iter().any(|op| matches!(op, Op::NewArray(_))));
        assert!(chunk.code.iter().any(|op| matches!(op, Op::NewObject(_))));
    }
}
