use murkiu_bytecode::*;
use std::collections::HashMap;
use std::fmt;

/// A JavaScript value in the VM.
#[derive(Clone)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Object(ObjectId),
    Function(FunctionValue),
    NativeFunction(NativeFn),
    Array(Vec<JsValue>),
}

impl fmt::Debug for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsValue::Undefined => write!(f, "undefined"),
            JsValue::Null => write!(f, "null"),
            JsValue::Bool(b) => write!(f, "{b}"),
            JsValue::Number(n) => write!(f, "{n}"),
            JsValue::Str(s) => write!(f, "\"{s}\""),
            JsValue::Object(id) => write!(f, "[Object #{id}]"),
            JsValue::Function(fv) => write!(
                f,
                "[Function {}]",
                fv.proto.name.as_deref().unwrap_or("anonymous")
            ),
            JsValue::NativeFunction(_) => write!(f, "[NativeFunction]"),
            JsValue::Array(arr) => write!(f, "[Array({})]", arr.len()),
        }
    }
}

impl fmt::Display for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsValue::Undefined => write!(f, "undefined"),
            JsValue::Null => write!(f, "null"),
            JsValue::Bool(b) => write!(f, "{b}"),
            JsValue::Number(n) => {
                if *n == f64::INFINITY {
                    write!(f, "Infinity")
                } else if *n == f64::NEG_INFINITY {
                    write!(f, "-Infinity")
                } else if n.is_nan() {
                    write!(f, "NaN")
                } else if *n == 0.0 && n.is_sign_negative() {
                    write!(f, "0")
                } else if n.fract() == 0.0 && n.abs() < 1e15 {
                    write!(f, "{}", *n as i64)
                } else {
                    write!(f, "{n}")
                }
            }
            JsValue::Str(s) => write!(f, "{s}"),
            JsValue::Object(_) => write!(f, "[object Object]"),
            JsValue::Function(fv) => {
                let name = fv.proto.name.as_deref().unwrap_or("anonymous");
                write!(f, "function {name}() {{ [native code] }}")
            }
            JsValue::NativeFunction(_) => write!(f, "function() {{ [native code] }}"),
            JsValue::Array(arr) => {
                let parts: Vec<String> = arr.iter().map(|v| format!("{v}")).collect();
                write!(f, "{}", parts.join(","))
            }
        }
    }
}

impl JsValue {
    pub fn is_truthy(&self) -> bool {
        match self {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Bool(b) => *b,
            JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
            JsValue::Str(s) => !s.is_empty(),
            _ => true,
        }
    }

    pub fn is_nullish(&self) -> bool {
        matches!(self, JsValue::Undefined | JsValue::Null)
    }

    pub fn to_number(&self) -> f64 {
        match self {
            JsValue::Undefined => f64::NAN,
            JsValue::Null => 0.0,
            JsValue::Bool(true) => 1.0,
            JsValue::Bool(false) => 0.0,
            JsValue::Number(n) => *n,
            JsValue::Str(s) => s.parse::<f64>().unwrap_or(f64::NAN),
            _ => f64::NAN,
        }
    }

    pub fn to_string_val(&self) -> String {
        format!("{self}")
    }

    pub fn typeof_str(&self) -> &'static str {
        match self {
            JsValue::Undefined => "undefined",
            JsValue::Null => "object",
            JsValue::Bool(_) => "boolean",
            JsValue::Number(_) => "number",
            JsValue::Str(_) => "string",
            JsValue::Object(_) => "object",
            JsValue::Function(_) | JsValue::NativeFunction(_) => "function",
            JsValue::Array(_) => "object",
        }
    }

    pub fn strict_eq(&self, other: &JsValue) -> bool {
        match (self, other) {
            (JsValue::Undefined, JsValue::Undefined) => true,
            (JsValue::Null, JsValue::Null) => true,
            (JsValue::Bool(a), JsValue::Bool(b)) => a == b,
            (JsValue::Number(a), JsValue::Number(b)) => a == b,
            (JsValue::Str(a), JsValue::Str(b)) => a == b,
            _ => false,
        }
    }

    pub fn abstract_eq(&self, other: &JsValue) -> bool {
        match (self, other) {
            (JsValue::Null, JsValue::Undefined) | (JsValue::Undefined, JsValue::Null) => true,
            (JsValue::Number(a), JsValue::Number(b)) => a == b,
            (JsValue::Str(a), JsValue::Str(b)) => a == b,
            (JsValue::Bool(a), JsValue::Bool(b)) => a == b,
            (JsValue::Number(_), JsValue::Str(s)) => {
                self.abstract_eq(&JsValue::Number(s.parse().unwrap_or(f64::NAN)))
            }
            (JsValue::Str(_), JsValue::Number(_)) => other.abstract_eq(self),
            (JsValue::Bool(b), _) => JsValue::Number(if *b { 1.0 } else { 0.0 }).abstract_eq(other),
            (_, JsValue::Bool(b)) => self.abstract_eq(&JsValue::Number(if *b { 1.0 } else { 0.0 })),
            _ => self.strict_eq(other),
        }
    }
}

pub type ObjectId = usize;
pub type NativeFn = fn(&mut Vm, Vec<JsValue>) -> JsValue;

#[derive(Clone)]
pub struct FunctionValue {
    pub proto: FunctionProto,
}

/// A heap-allocated JS object.
#[derive(Debug, Clone)]
pub struct JsObject {
    pub properties: HashMap<String, JsValue>,
    pub prototype: Option<ObjectId>,
    pub marked: bool,
}

impl JsObject {
    fn new() -> Self {
        JsObject {
            properties: HashMap::new(),
            prototype: None,
            marked: false,
        }
    }
}

/// A call frame on the call stack.
struct CallFrame {
    code: Vec<Op>,
    constants: Vec<Constant>,
    ip: usize,
    stack_base: usize,
    locals: Vec<JsValue>,
}

/// Exception handler on the try stack.
#[allow(dead_code)]
struct TryHandler {
    catch_ip: usize,
    finally_ip: usize,
    stack_base: usize,
    frame_depth: usize,
}

/// Captured console output for testing.
#[derive(Debug, Default, Clone)]
pub struct ConsoleOutput {
    pub lines: Vec<String>,
}

/// The Murkiu JavaScript virtual machine.
pub struct Vm {
    stack: Vec<JsValue>,
    frames: Vec<CallFrame>,
    pub globals: HashMap<String, JsValue>,
    pub heap: Vec<JsObject>,
    try_stack: Vec<TryHandler>,
    pub console_output: ConsoleOutput,
    gc_threshold: usize,
    /// The receiver object for the most recent property access that yielded a function.
    /// Used by native functions to access `this`.
    pub this_value: JsValue,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Self {
        let mut vm = Vm {
            stack: Vec::with_capacity(256),
            frames: Vec::new(),
            globals: HashMap::new(),
            heap: Vec::new(),
            try_stack: Vec::new(),
            console_output: ConsoleOutput::default(),
            gc_threshold: 256,
            this_value: JsValue::Undefined,
        };
        vm.init_globals();
        vm
    }

    fn init_globals(&mut self) {
        // console object
        let console_id = self.alloc_object();
        {
            let log_fn = JsValue::NativeFunction(native_console_log);
            let warn_fn = JsValue::NativeFunction(native_console_warn);
            let error_fn = JsValue::NativeFunction(native_console_error);
            self.heap[console_id]
                .properties
                .insert("log".into(), log_fn);
            self.heap[console_id]
                .properties
                .insert("warn".into(), warn_fn);
            self.heap[console_id]
                .properties
                .insert("error".into(), error_fn);
        }
        self.globals
            .insert("console".into(), JsValue::Object(console_id));

        // Math object
        let math_id = self.alloc_object();
        {
            self.heap[math_id]
                .properties
                .insert("PI".into(), JsValue::Number(std::f64::consts::PI));
            self.heap[math_id]
                .properties
                .insert("E".into(), JsValue::Number(std::f64::consts::E));
            self.heap[math_id]
                .properties
                .insert("floor".into(), JsValue::NativeFunction(native_math_floor));
            self.heap[math_id]
                .properties
                .insert("ceil".into(), JsValue::NativeFunction(native_math_ceil));
            self.heap[math_id]
                .properties
                .insert("round".into(), JsValue::NativeFunction(native_math_round));
            self.heap[math_id]
                .properties
                .insert("abs".into(), JsValue::NativeFunction(native_math_abs));
            self.heap[math_id]
                .properties
                .insert("max".into(), JsValue::NativeFunction(native_math_max));
            self.heap[math_id]
                .properties
                .insert("min".into(), JsValue::NativeFunction(native_math_min));
            self.heap[math_id]
                .properties
                .insert("random".into(), JsValue::NativeFunction(native_math_random));
            self.heap[math_id]
                .properties
                .insert("sqrt".into(), JsValue::NativeFunction(native_math_sqrt));
            self.heap[math_id]
                .properties
                .insert("pow".into(), JsValue::NativeFunction(native_math_pow));
        }
        self.globals.insert("Math".into(), JsValue::Object(math_id));

        // Global functions
        self.globals
            .insert("parseInt".into(), JsValue::NativeFunction(native_parse_int));
        self.globals.insert(
            "parseFloat".into(),
            JsValue::NativeFunction(native_parse_float),
        );
        self.globals
            .insert("isNaN".into(), JsValue::NativeFunction(native_is_nan));
        self.globals
            .insert("isFinite".into(), JsValue::NativeFunction(native_is_finite));
        self.globals.insert("NaN".into(), JsValue::Number(f64::NAN));
        self.globals
            .insert("Infinity".into(), JsValue::Number(f64::INFINITY));
        self.globals.insert("undefined".into(), JsValue::Undefined);

        // JSON object
        let json_id = self.alloc_object();
        self.heap[json_id]
            .properties
            .insert("parse".into(), JsValue::NativeFunction(native_json_parse));
        self.heap[json_id].properties.insert(
            "stringify".into(),
            JsValue::NativeFunction(native_json_stringify),
        );
        self.globals.insert("JSON".into(), JsValue::Object(json_id));

        // Object constructor
        let object_id = self.alloc_object();
        self.heap[object_id]
            .properties
            .insert("keys".into(), JsValue::NativeFunction(native_object_keys));
        self.heap[object_id].properties.insert(
            "values".into(),
            JsValue::NativeFunction(native_object_values),
        );
        self.heap[object_id].properties.insert(
            "entries".into(),
            JsValue::NativeFunction(native_object_entries),
        );
        self.heap[object_id].properties.insert(
            "assign".into(),
            JsValue::NativeFunction(native_object_assign),
        );
        self.globals
            .insert("Object".into(), JsValue::Object(object_id));

        // Array constructor
        let array_ctor_id = self.alloc_object();
        self.heap[array_ctor_id].properties.insert(
            "isArray".into(),
            JsValue::NativeFunction(native_array_is_array),
        );
        self.heap[array_ctor_id]
            .properties
            .insert("from".into(), JsValue::NativeFunction(native_array_from));
        self.globals
            .insert("Array".into(), JsValue::Object(array_ctor_id));

        // String constructor
        let string_ctor_id = self.alloc_object();
        self.heap[string_ctor_id].properties.insert(
            "fromCharCode".into(),
            JsValue::NativeFunction(native_string_from_char_code),
        );
        self.globals
            .insert("String".into(), JsValue::Object(string_ctor_id));

        // Timer stubs (no event loop — setTimeout just calls immediately)
        self.globals.insert(
            "setTimeout".into(),
            JsValue::NativeFunction(native_set_timeout),
        );
        self.globals.insert(
            "setInterval".into(),
            JsValue::NativeFunction(native_set_interval),
        );
        self.globals
            .insert("clearTimeout".into(), JsValue::NativeFunction(native_noop));
        self.globals
            .insert("clearInterval".into(), JsValue::NativeFunction(native_noop));
        self.globals.insert(
            "requestAnimationFrame".into(),
            JsValue::NativeFunction(native_noop),
        );
        self.globals.insert(
            "cancelAnimationFrame".into(),
            JsValue::NativeFunction(native_noop),
        );

        // Encoding
        self.globals.insert(
            "encodeURIComponent".into(),
            JsValue::NativeFunction(native_encode_uri_component),
        );
        self.globals.insert(
            "decodeURIComponent".into(),
            JsValue::NativeFunction(native_decode_uri_component),
        );
        self.globals.insert(
            "encodeURI".into(),
            JsValue::NativeFunction(native_encode_uri_component),
        );
        self.globals.insert(
            "decodeURI".into(),
            JsValue::NativeFunction(native_decode_uri_component),
        );
        self.globals
            .insert("atob".into(), JsValue::NativeFunction(native_noop));
        self.globals
            .insert("btoa".into(), JsValue::NativeFunction(native_noop));

        // Boolean/Number conversion
        self.globals
            .insert("Boolean".into(), JsValue::NativeFunction(native_boolean));
        self.globals
            .insert("Number".into(), JsValue::NativeFunction(native_number));
    }

    fn alloc_object(&mut self) -> ObjectId {
        // Simple GC trigger
        if self.heap.len() >= self.gc_threshold {
            self.gc();
        }
        let id = self.heap.len();
        self.heap.push(JsObject::new());
        id
    }

    fn push(&mut self, val: JsValue) {
        self.stack.push(val);
    }

    fn pop(&mut self) -> JsValue {
        self.stack.pop().unwrap_or(JsValue::Undefined)
    }

    fn peek(&self) -> &JsValue {
        self.stack.last().unwrap_or(&JsValue::Undefined)
    }

    /// Execute a compiled chunk.
    pub fn execute(&mut self, chunk: &Chunk) -> Result<JsValue, String> {
        self.frames.push(CallFrame {
            code: chunk.code.clone(),
            constants: chunk.constants.clone(),
            ip: 0,
            stack_base: 0,
            locals: vec![JsValue::Undefined; chunk.num_locals as usize],
        });

        self.run()
    }

    /// Parse, compile, and execute JS source.
    pub fn eval(&mut self, source: &str) -> Result<JsValue, String> {
        let program = murkiu_parser::parse(source)?;
        let chunk = murkiu_bytecode::compile(&program)?;
        self.execute(&chunk)
    }

    fn current_frame(&self) -> &CallFrame {
        self.frames.last().expect("VM: no active call frame")
    }

    fn current_frame_mut(&mut self) -> &mut CallFrame {
        self.frames.last_mut().expect("VM: no active call frame")
    }

    fn read_op(&mut self) -> Op {
        let frame = match self.frames.last_mut() {
            Some(f) => f,
            None => return Op::Halt,
        };
        if frame.ip >= frame.code.len() {
            return Op::Halt;
        }
        let op = frame.code[frame.ip].clone();
        frame.ip += 1;
        op
    }

    fn get_constant(&self, idx: u16) -> &Constant {
        &self.current_frame().constants[idx as usize]
    }

    fn run(&mut self) -> Result<JsValue, String> {
        let mut iteration_limit = 10_000_000u64; // prevent infinite loops
        loop {
            iteration_limit -= 1;
            if iteration_limit == 0 {
                return Err("Execution limit exceeded".into());
            }
            if self.frames.is_empty() {
                return Ok(self.pop());
            }
            let op = self.read_op();
            match op {
                Op::Halt => {
                    self.frames.pop();
                    let result = if !self.stack.is_empty() {
                        self.pop()
                    } else {
                        JsValue::Undefined
                    };
                    return Ok(result);
                }

                Op::Const(idx) => {
                    let val = match self.get_constant(idx).clone() {
                        Constant::Number(n) => JsValue::Number(n),
                        Constant::Str(s) => JsValue::Str(s),
                        Constant::Bool(b) => JsValue::Bool(b),
                        Constant::Null => JsValue::Null,
                        Constant::Undefined => JsValue::Undefined,
                        Constant::Function(proto) => JsValue::Function(FunctionValue { proto }),
                    };
                    self.push(val);
                }

                Op::Undefined => self.push(JsValue::Undefined),
                Op::Null => self.push(JsValue::Null),
                Op::True => self.push(JsValue::Bool(true)),
                Op::False => self.push(JsValue::Bool(false)),
                Op::This => self.push(JsValue::Undefined), // simplified: no `this` binding yet

                // Arithmetic
                Op::Add => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (&a, &b) {
                        (JsValue::Str(sa), _) => JsValue::Str(format!("{sa}{b}")),
                        (_, JsValue::Str(sb)) => JsValue::Str(format!("{a}{sb}")),
                        _ => JsValue::Number(a.to_number() + b.to_number()),
                    };
                    self.push(result);
                }
                Op::Sub => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(a.to_number() - b.to_number()));
                }
                Op::Mul => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(a.to_number() * b.to_number()));
                }
                Op::Div => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(a.to_number() / b.to_number()));
                }
                Op::Mod => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(a.to_number() % b.to_number()));
                }
                Op::Pow => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(a.to_number().powf(b.to_number())));
                }
                Op::Neg => {
                    let a = self.pop();
                    self.push(JsValue::Number(-a.to_number()));
                }
                Op::Pos => {
                    let a = self.pop();
                    self.push(JsValue::Number(a.to_number()));
                }
                Op::BitNot => {
                    let a = self.pop();
                    self.push(JsValue::Number(!(a.to_number() as i32) as f64));
                }
                Op::BitAnd => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(
                        ((a.to_number() as i32) & (b.to_number() as i32)) as f64,
                    ));
                }
                Op::BitOr => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(
                        ((a.to_number() as i32) | (b.to_number() as i32)) as f64,
                    ));
                }
                Op::BitXor => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(
                        ((a.to_number() as i32) ^ (b.to_number() as i32)) as f64,
                    ));
                }
                Op::Shl => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(
                        ((a.to_number() as i32) << ((b.to_number() as u32) & 0x1f)) as f64,
                    ));
                }
                Op::Shr => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(
                        ((a.to_number() as i32) >> ((b.to_number() as u32) & 0x1f)) as f64,
                    ));
                }
                Op::UShr => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Number(
                        ((a.to_number() as u32) >> ((b.to_number() as u32) & 0x1f)) as f64,
                    ));
                }

                // Comparison
                Op::Eq => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Bool(a.abstract_eq(&b)));
                }
                Op::Ne => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Bool(!a.abstract_eq(&b)));
                }
                Op::StrictEq => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Bool(a.strict_eq(&b)));
                }
                Op::StrictNe => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Bool(!a.strict_eq(&b)));
                }
                Op::Lt => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Bool(a.to_number() < b.to_number()));
                }
                Op::Gt => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Bool(a.to_number() > b.to_number()));
                }
                Op::Le => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Bool(a.to_number() <= b.to_number()));
                }
                Op::Ge => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::Bool(a.to_number() >= b.to_number()));
                }
                Op::Instanceof => {
                    self.pop();
                    self.pop();
                    self.push(JsValue::Bool(false));
                } // simplified
                Op::In => {
                    self.pop();
                    self.pop();
                    self.push(JsValue::Bool(false));
                } // simplified

                // Logical / unary
                Op::Not => {
                    let a = self.pop();
                    self.push(JsValue::Bool(!a.is_truthy()));
                }
                Op::Typeof => {
                    let a = self.pop();
                    self.push(JsValue::Str(a.typeof_str().to_string()));
                }
                Op::Void => {
                    self.pop();
                    self.push(JsValue::Undefined);
                }
                Op::Delete => {
                    self.pop();
                    self.push(JsValue::Bool(true));
                } // simplified

                // Variables
                Op::GetLocal(slot) => {
                    let val = self.current_frame().locals[slot as usize].clone();
                    self.push(val);
                }
                Op::SetLocal(slot) => {
                    let val = self.peek().clone();
                    self.current_frame_mut().locals[slot as usize] = val;
                }
                Op::GetGlobal(idx) => {
                    let name = match self.get_constant(idx).clone() {
                        Constant::Str(s) => s,
                        _ => return Err("GetGlobal: expected string constant".into()),
                    };
                    let val = self
                        .globals
                        .get(&name)
                        .cloned()
                        .unwrap_or(JsValue::Undefined);
                    self.push(val);
                }
                Op::SetGlobal(idx) => {
                    let name = match self.get_constant(idx).clone() {
                        Constant::Str(s) => s,
                        _ => return Err("SetGlobal: expected string constant".into()),
                    };
                    let val = self.peek().clone();
                    self.globals.insert(name, val);
                }

                // Properties
                Op::GetField(idx) => {
                    let name = match self.get_constant(idx).clone() {
                        Constant::Str(s) => s,
                        _ => return Err("GetField: expected string constant".into()),
                    };
                    let obj = self.pop();
                    let val = self.get_property(&obj, &name);
                    // Track method receiver so native functions can access `this`
                    match &val {
                        JsValue::Function(_) | JsValue::NativeFunction(_) => {
                            self.this_value = obj;
                        }
                        _ => {}
                    }
                    self.push(val);
                }
                Op::SetField(idx) => {
                    let name = match self.get_constant(idx).clone() {
                        Constant::Str(s) => s,
                        _ => return Err("SetField: expected string constant".into()),
                    };
                    let obj = self.pop();
                    let val = self.pop();
                    if let JsValue::Object(id) = obj {
                        self.heap[id].properties.insert(name, val.clone());
                    }
                    self.push(val);
                }
                Op::GetProp => {
                    let key = self.pop();
                    let obj = self.pop();
                    let name = key.to_string_val();
                    let val = self.get_property(&obj, &name);
                    // Track method receiver so native functions can access `this`
                    match &val {
                        JsValue::Function(_) | JsValue::NativeFunction(_) => {
                            self.this_value = obj;
                        }
                        _ => {}
                    }
                    self.push(val);
                }
                Op::SetProp => {
                    let val = self.pop();
                    let key = self.pop();
                    let obj = self.pop();
                    let name = key.to_string_val();
                    if let JsValue::Object(id) = obj {
                        self.heap[id].properties.insert(name, val.clone());
                    }
                    self.push(val);
                }

                // Control flow
                Op::Jump(offset) => {
                    let frame = self.current_frame_mut();
                    frame.ip = (frame.ip as i32 + offset) as usize;
                }
                Op::JumpIfFalse(offset) => {
                    let val = self.pop();
                    if !val.is_truthy() {
                        let frame = self.current_frame_mut();
                        frame.ip = (frame.ip as i32 + offset) as usize;
                    }
                }
                Op::JumpIfTrue(offset) => {
                    let val = self.pop();
                    if val.is_truthy() {
                        let frame = self.current_frame_mut();
                        frame.ip = (frame.ip as i32 + offset) as usize;
                    }
                }
                Op::JumpIfNullish(offset) => {
                    if self.peek().is_nullish() {
                        let frame = self.current_frame_mut();
                        frame.ip = (frame.ip as i32 + offset) as usize;
                    }
                }

                // Functions
                Op::Closure(idx) => {
                    let proto = match self.get_constant(idx).clone() {
                        Constant::Function(p) => p,
                        _ => return Err("Closure: expected function constant".into()),
                    };
                    self.push(JsValue::Function(FunctionValue { proto }));
                }

                Op::Call(argc) => {
                    let argc = argc as usize;
                    let mut args = Vec::with_capacity(argc);
                    for _ in 0..argc {
                        args.push(self.pop());
                    }
                    args.reverse();
                    let callee = self.pop();

                    match callee {
                        JsValue::Function(fv) => {
                            let mut locals = vec![JsValue::Undefined; fv.proto.num_locals as usize];
                            for (i, arg) in args.iter().enumerate() {
                                if i < locals.len() {
                                    locals[i] = arg.clone();
                                }
                            }
                            self.frames.push(CallFrame {
                                code: fv.proto.code.clone(),
                                constants: fv.proto.constants.clone(),
                                ip: 0,
                                stack_base: self.stack.len(),
                                locals,
                            });
                        }
                        JsValue::NativeFunction(f) => {
                            let result = f(self, args);
                            self.push(result);
                        }
                        _ => {
                            // In a real engine this would throw TypeError
                            self.push(JsValue::Undefined);
                        }
                    }
                }

                Op::NewCall(argc) => {
                    // Simplified: create new object, call constructor
                    let argc = argc as usize;
                    let mut args = Vec::with_capacity(argc);
                    for _ in 0..argc {
                        args.push(self.pop());
                    }
                    args.reverse();
                    let _callee = self.pop();
                    let obj_id = self.alloc_object();
                    self.push(JsValue::Object(obj_id));
                }

                Op::Return => {
                    let val = self.pop();
                    let frame = self.frames.pop().unwrap();
                    // Clean up stack to frame's base
                    self.stack.truncate(frame.stack_base);
                    if self.frames.is_empty() {
                        // Top-level return
                        return Ok(val);
                    }
                    self.push(val);
                }

                // Stack ops
                Op::Pop => {
                    self.pop();
                }
                Op::Dup => {
                    let val = self.peek().clone();
                    self.push(val);
                }
                Op::Swap => {
                    let len = self.stack.len();
                    if len >= 2 {
                        self.stack.swap(len - 1, len - 2);
                    }
                }

                // Objects / arrays
                Op::NewArray(n) => {
                    let n = n as usize;
                    let mut elems = Vec::with_capacity(n);
                    for _ in 0..n {
                        elems.push(self.pop());
                    }
                    elems.reverse();
                    self.push(JsValue::Array(elems));
                }

                Op::NewObject(n) => {
                    let n = n as usize;
                    let obj_id = self.alloc_object();
                    // Stack has pairs: [key0, val0, key1, val1, ...]
                    let mut pairs = Vec::with_capacity(n);
                    for _ in 0..n {
                        let val = self.pop();
                        let key = self.pop();
                        pairs.push((key.to_string_val(), val));
                    }
                    for (k, v) in pairs {
                        self.heap[obj_id].properties.insert(k, v);
                    }
                    self.push(JsValue::Object(obj_id));
                }

                // Increment / Decrement
                Op::PreInc => {
                    let a = self.pop();
                    self.push(JsValue::Number(a.to_number() + 1.0));
                }
                Op::PreDec => {
                    let a = self.pop();
                    self.push(JsValue::Number(a.to_number() - 1.0));
                }
                Op::PostInc => {
                    let a = self.pop();
                    let n = a.to_number();
                    self.push(JsValue::Number(n)); // push original
                                                   // Note: in a full impl we'd store n+1 back to the variable
                }
                Op::PostDec => {
                    let a = self.pop();
                    let n = a.to_number();
                    self.push(JsValue::Number(n)); // push original
                }

                // Exception handling (simplified)
                Op::Throw => {
                    let val = self.pop();
                    if let Some(handler) = self.try_stack.pop() {
                        // Jump to catch block
                        self.current_frame_mut().ip = handler.catch_ip;
                        self.push(val); // push exception as catch parameter
                    } else {
                        return Err(format!("Uncaught: {val}"));
                    }
                }
                Op::EnterTry(catch_off, _finally_off) => {
                    let frame = self.current_frame();
                    let base_ip = frame.ip; // ip already advanced past EnterTry
                    let catch_ip = (base_ip as i32 + catch_off) as usize;
                    self.try_stack.push(TryHandler {
                        catch_ip,
                        finally_ip: 0,
                        stack_base: self.stack.len(),
                        frame_depth: self.frames.len(),
                    });
                }
                Op::LeaveTry => {
                    self.try_stack.pop();
                }

                Op::Debugger => {} // no-op
            }
        }
    }

    /// Call a JS function value with the given arguments. Used by native functions
    /// that need to invoke callbacks (e.g. Array.map, Array.filter).
    pub fn call_function(&mut self, func: &JsValue, args: Vec<JsValue>) -> JsValue {
        match func {
            JsValue::Function(fv) => {
                let mut locals = vec![JsValue::Undefined; fv.proto.num_locals as usize];
                for (i, arg) in args.iter().enumerate() {
                    if i < locals.len() {
                        locals[i] = arg.clone();
                    }
                }
                self.frames.push(CallFrame {
                    code: fv.proto.code.clone(),
                    constants: fv.proto.constants.clone(),
                    ip: 0,
                    stack_base: self.stack.len(),
                    locals,
                });
                match self.run() {
                    Ok(val) => val,
                    Err(_) => JsValue::Undefined,
                }
            }
            JsValue::NativeFunction(f) => f(self, args),
            _ => JsValue::Undefined,
        }
    }

    fn get_property(&self, obj: &JsValue, name: &str) -> JsValue {
        match obj {
            JsValue::Object(id) => {
                if let Some(val) = self.heap[*id].properties.get(name) {
                    val.clone()
                } else {
                    JsValue::Undefined
                }
            }
            JsValue::Str(s) => {
                match name {
                    "length" => JsValue::Number(s.chars().count() as f64),
                    // String methods — return native functions
                    "split" => JsValue::NativeFunction(native_string_split),
                    "indexOf" => JsValue::NativeFunction(native_string_index_of),
                    "lastIndexOf" => JsValue::NativeFunction(native_string_last_index_of),
                    "includes" => JsValue::NativeFunction(native_string_includes),
                    "substring" => JsValue::NativeFunction(native_string_substring),
                    "slice" => JsValue::NativeFunction(native_string_slice),
                    "toUpperCase" => JsValue::NativeFunction(native_string_to_upper),
                    "toLowerCase" => JsValue::NativeFunction(native_string_to_lower),
                    "trim" => JsValue::NativeFunction(native_string_trim),
                    "trimStart" => JsValue::NativeFunction(native_string_trim_start),
                    "trimEnd" => JsValue::NativeFunction(native_string_trim_end),
                    "replace" => JsValue::NativeFunction(native_string_replace),
                    "replaceAll" => JsValue::NativeFunction(native_string_replace_all),
                    "charAt" => JsValue::NativeFunction(native_string_char_at),
                    "charCodeAt" => JsValue::NativeFunction(native_string_char_code_at),
                    "startsWith" => JsValue::NativeFunction(native_string_starts_with),
                    "endsWith" => JsValue::NativeFunction(native_string_ends_with),
                    "repeat" => JsValue::NativeFunction(native_string_repeat),
                    "padStart" => JsValue::NativeFunction(native_string_pad_start),
                    "padEnd" => JsValue::NativeFunction(native_string_pad_end),
                    "concat" => JsValue::NativeFunction(native_string_concat),
                    "match" => JsValue::NativeFunction(native_string_match),
                    "search" => JsValue::NativeFunction(native_string_search),
                    "toString" => JsValue::NativeFunction(native_string_to_string),
                    "valueOf" => JsValue::NativeFunction(native_string_to_string),
                    _ => {
                        // Numeric index access
                        if let Ok(idx) = name.parse::<usize>() {
                            s.chars()
                                .nth(idx)
                                .map(|c| JsValue::Str(c.to_string()))
                                .unwrap_or(JsValue::Undefined)
                        } else {
                            JsValue::Undefined
                        }
                    }
                }
            }
            JsValue::Array(arr) => {
                match name {
                    "length" => JsValue::Number(arr.len() as f64),
                    // Array methods
                    "push" => JsValue::NativeFunction(native_array_push),
                    "pop" => JsValue::NativeFunction(native_array_pop),
                    "shift" => JsValue::NativeFunction(native_array_shift),
                    "unshift" => JsValue::NativeFunction(native_array_unshift),
                    "map" => JsValue::NativeFunction(native_array_map),
                    "filter" => JsValue::NativeFunction(native_array_filter),
                    "forEach" => JsValue::NativeFunction(native_array_for_each),
                    "indexOf" => JsValue::NativeFunction(native_array_index_of),
                    "lastIndexOf" => JsValue::NativeFunction(native_array_last_index_of),
                    "find" => JsValue::NativeFunction(native_array_find),
                    "findIndex" => JsValue::NativeFunction(native_array_find_index),
                    "some" => JsValue::NativeFunction(native_array_some),
                    "every" => JsValue::NativeFunction(native_array_every),
                    "includes" => JsValue::NativeFunction(native_array_includes),
                    "join" => JsValue::NativeFunction(native_array_join),
                    "slice" => JsValue::NativeFunction(native_array_slice),
                    "concat" => JsValue::NativeFunction(native_array_concat),
                    "splice" => JsValue::NativeFunction(native_array_splice),
                    "reverse" => JsValue::NativeFunction(native_array_reverse),
                    "sort" => JsValue::NativeFunction(native_array_sort),
                    "reduce" => JsValue::NativeFunction(native_array_reduce),
                    "flat" => JsValue::NativeFunction(native_array_flat),
                    "fill" => JsValue::NativeFunction(native_array_fill),
                    "toString" => JsValue::NativeFunction(native_array_to_string),
                    _ => {
                        if let Ok(idx) = name.parse::<usize>() {
                            arr.get(idx).cloned().unwrap_or(JsValue::Undefined)
                        } else {
                            JsValue::Undefined
                        }
                    }
                }
            }
            _ => JsValue::Undefined,
        }
    }

    // --- Garbage Collector (mark and sweep) ---

    fn gc(&mut self) {
        // Mark phase: mark all reachable objects
        for obj in &mut self.heap {
            obj.marked = false;
        }

        // Collect all root object IDs first (to avoid borrow conflicts)
        let mut worklist: Vec<usize> = Vec::new();
        Self::collect_obj_ids(&self.stack, &mut worklist);
        let global_vals: Vec<JsValue> = self.globals.values().cloned().collect();
        Self::collect_obj_ids(&global_vals, &mut worklist);
        let frame_locals: Vec<JsValue> = self
            .frames
            .iter()
            .flat_map(|f| f.locals.iter().cloned())
            .collect();
        Self::collect_obj_ids(&frame_locals, &mut worklist);

        // Mark using worklist
        while let Some(id) = worklist.pop() {
            if id >= self.heap.len() || self.heap[id].marked {
                continue;
            }
            self.heap[id].marked = true;
            let prop_vals: Vec<JsValue> = self.heap[id].properties.values().cloned().collect();
            Self::collect_obj_ids(&prop_vals, &mut worklist);
        }

        // Sweep: we don't actually free individual objects since we use a Vec arena.
        // Just update the threshold.
        let alive = self.heap.iter().filter(|o| o.marked).count();
        self.gc_threshold = (alive * 2).max(256);
    }

    fn collect_obj_ids(values: &[JsValue], out: &mut Vec<usize>) {
        for val in values {
            match val {
                JsValue::Object(id) => out.push(*id),
                JsValue::Array(arr) => Self::collect_obj_ids(arr, out),
                _ => {}
            }
        }
    }
}

// --- Native functions ---

fn native_console_log(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let line: String = args
        .iter()
        .map(|a| format!("{a}"))
        .collect::<Vec<_>>()
        .join(" ");
    vm.console_output.lines.push(line.clone());
    log::info!("[console.log] {line}");
    JsValue::Undefined
}

fn native_console_warn(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let line: String = args
        .iter()
        .map(|a| format!("{a}"))
        .collect::<Vec<_>>()
        .join(" ");
    vm.console_output.lines.push(format!("[warn] {line}"));
    log::warn!("[console.warn] {line}");
    JsValue::Undefined
}

fn native_console_error(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let line: String = args
        .iter()
        .map(|a| format!("{a}"))
        .collect::<Vec<_>>()
        .join(" ");
    vm.console_output.lines.push(format!("[error] {line}"));
    log::error!("[console.error] {line}");
    JsValue::Undefined
}

fn native_math_floor(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let n = args.first().map(|a| a.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(n.floor())
}

fn native_math_ceil(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let n = args.first().map(|a| a.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(n.ceil())
}

fn native_math_round(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let n = args.first().map(|a| a.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(n.round())
}

fn native_math_abs(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let n = args.first().map(|a| a.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(n.abs())
}

fn native_math_max(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    if args.is_empty() {
        return JsValue::Number(f64::NEG_INFINITY);
    }
    let mut max = f64::NEG_INFINITY;
    for arg in &args {
        let n = arg.to_number();
        if n.is_nan() {
            return JsValue::Number(f64::NAN);
        }
        if n > max {
            max = n;
        }
    }
    JsValue::Number(max)
}

fn native_math_min(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    if args.is_empty() {
        return JsValue::Number(f64::INFINITY);
    }
    let mut min = f64::INFINITY;
    for arg in &args {
        let n = arg.to_number();
        if n.is_nan() {
            return JsValue::Number(f64::NAN);
        }
        if n < min {
            min = n;
        }
    }
    JsValue::Number(min)
}

fn native_math_random(_vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    // Simple LCG random - not cryptographically secure, but fine for JS Math.random()
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    let val = ((seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407))
        >> 33) as f64
        / (1u64 << 31) as f64;
    JsValue::Number(val)
}

fn native_math_sqrt(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let n = args.first().map(|a| a.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(n.sqrt())
}

fn native_math_pow(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let base = args.first().map(|a| a.to_number()).unwrap_or(f64::NAN);
    let exp = args.get(1).map(|a| a.to_number()).unwrap_or(f64::NAN);
    JsValue::Number(base.powf(exp))
}

fn native_parse_int(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    let s = s.trim();
    match s.parse::<i64>() {
        Ok(n) => JsValue::Number(n as f64),
        Err(_) => JsValue::Number(f64::NAN),
    }
}

fn native_parse_float(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    match s.trim().parse::<f64>() {
        Ok(n) => JsValue::Number(n),
        Err(_) => JsValue::Number(f64::NAN),
    }
}

fn native_is_nan(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let n = args.first().map(|a| a.to_number()).unwrap_or(f64::NAN);
    JsValue::Bool(n.is_nan())
}

fn native_is_finite(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let n = args.first().map(|a| a.to_number()).unwrap_or(f64::NAN);
    JsValue::Bool(n.is_finite())
}

fn native_noop(_vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    JsValue::Undefined
}

fn native_set_timeout(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    // No event loop — just call the function immediately
    if let Some(func) = args.first() {
        vm.call_function(func, vec![]);
    }
    JsValue::Number(0.0)
}

fn native_set_interval(_vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    JsValue::Number(0.0) // no-op
}

fn native_boolean(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let val = args.first().cloned().unwrap_or(JsValue::Undefined);
    JsValue::Bool(val.is_truthy())
}

fn native_number(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let val = args.first().cloned().unwrap_or(JsValue::Undefined);
    JsValue::Number(val.to_number())
}

fn native_encode_uri_component(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    let mut encoded = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'!'
            | b'\''
            | b'('
            | b')'
            | b'*' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    JsValue::Str(encoded)
}

fn native_decode_uri_component(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(val) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                result.push(val);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    JsValue::Str(String::from_utf8_lossy(&result).to_string())
}

// ─── String native methods ───────────────────────────────────────────────

fn get_this_string(vm: &Vm) -> String {
    match &vm.this_value {
        JsValue::Str(s) => s.clone(),
        other => other.to_string_val(),
    }
}

fn native_string_split(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let sep = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    let limit = args.get(1).and_then(|a| match a {
        JsValue::Number(n) => Some(*n as usize),
        _ => None,
    });
    let parts: Vec<JsValue> = if sep.is_empty() {
        s.chars().map(|c| JsValue::Str(c.to_string())).collect()
    } else {
        s.split(&sep).map(|p| JsValue::Str(p.to_string())).collect()
    };
    let parts = match limit {
        Some(lim) => parts.into_iter().take(lim).collect(),
        None => parts,
    };
    JsValue::Array(parts)
}

fn native_string_index_of(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let search = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    let from = args.get(1).map(|a| a.to_number() as usize).unwrap_or(0);
    match s[from.min(s.len())..].find(&search) {
        Some(pos) => JsValue::Number((pos + from) as f64),
        None => JsValue::Number(-1.0),
    }
}

fn native_string_last_index_of(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let search = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    match s.rfind(&search) {
        Some(pos) => JsValue::Number(pos as f64),
        None => JsValue::Number(-1.0),
    }
}

fn native_string_includes(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let search = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    JsValue::Bool(s.contains(&search))
}

fn native_string_substring(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let len = s.len();
    let start = args
        .first()
        .map(|a| (a.to_number() as usize).min(len))
        .unwrap_or(0);
    let end = args
        .get(1)
        .map(|a| (a.to_number() as usize).min(len))
        .unwrap_or(len);
    let (start, end) = if start > end {
        (end, start)
    } else {
        (start, end)
    };
    JsValue::Str(s.get(start..end).unwrap_or("").to_string())
}

fn native_string_slice(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let len = s.len() as i64;
    let start = args
        .first()
        .map(|a| {
            let n = a.to_number() as i64;
            if n < 0 {
                (len + n).max(0) as usize
            } else {
                n.min(len) as usize
            }
        })
        .unwrap_or(0);
    let end = args
        .get(1)
        .map(|a| {
            let n = a.to_number() as i64;
            if n < 0 {
                (len + n).max(0) as usize
            } else {
                n.min(len) as usize
            }
        })
        .unwrap_or(len as usize);
    if start >= end {
        JsValue::Str(String::new())
    } else {
        JsValue::Str(s.get(start..end).unwrap_or("").to_string())
    }
}

fn native_string_to_upper(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    JsValue::Str(get_this_string(vm).to_uppercase())
}

fn native_string_to_lower(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    JsValue::Str(get_this_string(vm).to_lowercase())
}

fn native_string_trim(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    JsValue::Str(get_this_string(vm).trim().to_string())
}

fn native_string_trim_start(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    JsValue::Str(get_this_string(vm).trim_start().to_string())
}

fn native_string_trim_end(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    JsValue::Str(get_this_string(vm).trim_end().to_string())
}

fn native_string_replace(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let search = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    let replace = args.get(1).map(|a| a.to_string_val()).unwrap_or_default();
    JsValue::Str(s.replacen(&search, &replace, 1))
}

fn native_string_replace_all(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let search = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    let replace = args.get(1).map(|a| a.to_string_val()).unwrap_or_default();
    JsValue::Str(s.replace(&search, &replace))
}

fn native_string_char_at(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let idx = args.first().map(|a| a.to_number() as usize).unwrap_or(0);
    s.chars()
        .nth(idx)
        .map(|c| JsValue::Str(c.to_string()))
        .unwrap_or(JsValue::Str(String::new()))
}

fn native_string_char_code_at(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let idx = args.first().map(|a| a.to_number() as usize).unwrap_or(0);
    s.chars()
        .nth(idx)
        .map(|c| JsValue::Number(c as u32 as f64))
        .unwrap_or(JsValue::Number(f64::NAN))
}

fn native_string_starts_with(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let search = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    JsValue::Bool(s.starts_with(&search))
}

fn native_string_ends_with(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let search = args.first().map(|a| a.to_string_val()).unwrap_or_default();
    JsValue::Bool(s.ends_with(&search))
}

fn native_string_repeat(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let count = args.first().map(|a| a.to_number() as usize).unwrap_or(0);
    JsValue::Str(s.repeat(count.min(10000))) // limit to prevent DoS
}

fn native_string_pad_start(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let target_len = args.first().map(|a| a.to_number() as usize).unwrap_or(0);
    let pad_str = args
        .get(1)
        .map(|a| a.to_string_val())
        .unwrap_or_else(|| " ".to_string());
    if s.len() >= target_len || pad_str.is_empty() {
        return JsValue::Str(s);
    }
    let needed = target_len - s.len();
    let pad = pad_str.repeat((needed / pad_str.len()) + 1);
    JsValue::Str(format!("{}{}", &pad[..needed], s))
}

fn native_string_pad_end(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = get_this_string(vm);
    let target_len = args.first().map(|a| a.to_number() as usize).unwrap_or(0);
    let pad_str = args
        .get(1)
        .map(|a| a.to_string_val())
        .unwrap_or_else(|| " ".to_string());
    if s.len() >= target_len || pad_str.is_empty() {
        return JsValue::Str(s);
    }
    let needed = target_len - s.len();
    let pad = pad_str.repeat((needed / pad_str.len()) + 1);
    JsValue::Str(format!("{}{}", s, &pad[..needed]))
}

fn native_string_concat(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let mut s = get_this_string(vm);
    for arg in &args {
        s.push_str(&arg.to_string_val());
    }
    JsValue::Str(s)
}

fn native_string_match(_vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    JsValue::Null // no regex support
}

fn native_string_search(_vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    JsValue::Number(-1.0) // no regex support
}

fn native_string_to_string(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    JsValue::Str(get_this_string(vm))
}

fn native_string_from_char_code(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let mut s = String::new();
    for arg in &args {
        let code = arg.to_number() as u32;
        if let Some(c) = char::from_u32(code) {
            s.push(c);
        }
    }
    JsValue::Str(s)
}

// ─── Array native methods ───────────────────────────────────────────────

fn get_this_array(vm: &Vm) -> Vec<JsValue> {
    match &vm.this_value {
        JsValue::Array(arr) => arr.clone(),
        _ => vec![],
    }
}

fn native_array_push(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let mut arr = get_this_array(vm);
    for arg in args {
        arr.push(arg);
    }
    let len = arr.len();
    vm.this_value = JsValue::Array(arr);
    JsValue::Number(len as f64)
}

fn native_array_pop(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    let mut arr = get_this_array(vm);
    let val = arr.pop().unwrap_or(JsValue::Undefined);
    vm.this_value = JsValue::Array(arr);
    val
}

fn native_array_shift(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    let mut arr = get_this_array(vm);
    let val = if arr.is_empty() {
        JsValue::Undefined
    } else {
        arr.remove(0)
    };
    vm.this_value = JsValue::Array(arr);
    val
}

fn native_array_unshift(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let mut arr = get_this_array(vm);
    for (i, arg) in args.into_iter().enumerate() {
        arr.insert(i, arg);
    }
    let len = arr.len();
    vm.this_value = JsValue::Array(arr);
    JsValue::Number(len as f64)
}

fn native_array_map(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let func = match args.first() {
        Some(f) => f.clone(),
        None => return JsValue::Array(arr),
    };
    let mut result = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let val = vm.call_function(&func, vec![item.clone(), JsValue::Number(i as f64)]);
        result.push(val);
    }
    JsValue::Array(result)
}

fn native_array_filter(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let func = match args.first() {
        Some(f) => f.clone(),
        None => return JsValue::Array(arr),
    };
    let mut result = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let val = vm.call_function(&func, vec![item.clone(), JsValue::Number(i as f64)]);
        if val.is_truthy() {
            result.push(item.clone());
        }
    }
    JsValue::Array(result)
}

fn native_array_for_each(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let func = match args.first() {
        Some(f) => f.clone(),
        None => return JsValue::Undefined,
    };
    for (i, item) in arr.iter().enumerate() {
        vm.call_function(&func, vec![item.clone(), JsValue::Number(i as f64)]);
    }
    JsValue::Undefined
}

fn native_array_index_of(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let search = args.first().cloned().unwrap_or(JsValue::Undefined);
    let from = args.get(1).map(|a| a.to_number() as usize).unwrap_or(0);
    for (i, item) in arr.iter().enumerate().skip(from) {
        if item.strict_eq(&search) {
            return JsValue::Number(i as f64);
        }
    }
    JsValue::Number(-1.0)
}

fn native_array_last_index_of(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let search = args.first().cloned().unwrap_or(JsValue::Undefined);
    for i in (0..arr.len()).rev() {
        if arr[i].strict_eq(&search) {
            return JsValue::Number(i as f64);
        }
    }
    JsValue::Number(-1.0)
}

fn native_array_find(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let func = match args.first() {
        Some(f) => f.clone(),
        None => return JsValue::Undefined,
    };
    for (i, item) in arr.iter().enumerate() {
        let val = vm.call_function(&func, vec![item.clone(), JsValue::Number(i as f64)]);
        if val.is_truthy() {
            return item.clone();
        }
    }
    JsValue::Undefined
}

fn native_array_find_index(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let func = match args.first() {
        Some(f) => f.clone(),
        None => return JsValue::Number(-1.0),
    };
    for (i, item) in arr.iter().enumerate() {
        let val = vm.call_function(&func, vec![item.clone(), JsValue::Number(i as f64)]);
        if val.is_truthy() {
            return JsValue::Number(i as f64);
        }
    }
    JsValue::Number(-1.0)
}

fn native_array_some(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let func = match args.first() {
        Some(f) => f.clone(),
        None => return JsValue::Bool(false),
    };
    for (i, item) in arr.iter().enumerate() {
        let val = vm.call_function(&func, vec![item.clone(), JsValue::Number(i as f64)]);
        if val.is_truthy() {
            return JsValue::Bool(true);
        }
    }
    JsValue::Bool(false)
}

fn native_array_every(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let func = match args.first() {
        Some(f) => f.clone(),
        None => return JsValue::Bool(true),
    };
    for (i, item) in arr.iter().enumerate() {
        let val = vm.call_function(&func, vec![item.clone(), JsValue::Number(i as f64)]);
        if !val.is_truthy() {
            return JsValue::Bool(false);
        }
    }
    JsValue::Bool(true)
}

fn native_array_includes(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let search = args.first().cloned().unwrap_or(JsValue::Undefined);
    JsValue::Bool(arr.iter().any(|v| v.strict_eq(&search)))
}

fn native_array_join(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let sep = args
        .first()
        .map(|a| a.to_string_val())
        .unwrap_or_else(|| ",".to_string());
    let parts: Vec<String> = arr.iter().map(|v| v.to_string_val()).collect();
    JsValue::Str(parts.join(&sep))
}

fn native_array_slice(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let len = arr.len() as i64;
    let start = args
        .first()
        .map(|a| {
            let n = a.to_number() as i64;
            if n < 0 {
                (len + n).max(0) as usize
            } else {
                n.min(len) as usize
            }
        })
        .unwrap_or(0);
    let end = args
        .get(1)
        .map(|a| {
            let n = a.to_number() as i64;
            if n < 0 {
                (len + n).max(0) as usize
            } else {
                n.min(len) as usize
            }
        })
        .unwrap_or(len as usize);
    if start >= end {
        JsValue::Array(vec![])
    } else {
        JsValue::Array(arr[start..end].to_vec())
    }
}

fn native_array_concat(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let mut arr = get_this_array(vm);
    for arg in args {
        match arg {
            JsValue::Array(other) => arr.extend(other),
            other => arr.push(other),
        }
    }
    JsValue::Array(arr)
}

fn native_array_splice(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let mut arr = get_this_array(vm);
    let len = arr.len() as i64;
    let start = args
        .first()
        .map(|a| {
            let n = a.to_number() as i64;
            if n < 0 {
                (len + n).max(0) as usize
            } else {
                n.min(len) as usize
            }
        })
        .unwrap_or(0);
    let delete_count = args
        .get(1)
        .map(|a| a.to_number() as usize)
        .unwrap_or(arr.len() - start);
    let delete_count = delete_count.min(arr.len() - start);
    let removed: Vec<JsValue> = arr.drain(start..start + delete_count).collect();
    // Insert new items
    for (i, arg) in args.iter().skip(2).enumerate() {
        arr.insert(start + i, arg.clone());
    }
    vm.this_value = JsValue::Array(arr);
    JsValue::Array(removed)
}

fn native_array_reverse(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    let mut arr = get_this_array(vm);
    arr.reverse();
    let result = JsValue::Array(arr.clone());
    vm.this_value = JsValue::Array(arr);
    result
}

fn native_array_sort(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let mut arr = get_this_array(vm);
    let compare_fn = args.first().cloned();
    if let Some(ref func) = compare_fn {
        let func = func.clone();
        // Simple insertion sort with comparison function
        for i in 1..arr.len() {
            let mut j = i;
            while j > 0 {
                let cmp = vm.call_function(&func, vec![arr[j - 1].clone(), arr[j].clone()]);
                if cmp.to_number() > 0.0 {
                    arr.swap(j - 1, j);
                    j -= 1;
                } else {
                    break;
                }
            }
        }
    } else {
        // Default: sort as strings
        arr.sort_by_key(|a| a.to_string_val());
    }
    let result = JsValue::Array(arr.clone());
    vm.this_value = JsValue::Array(arr);
    result
}

fn native_array_reduce(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let func = match args.first() {
        Some(f) => f.clone(),
        None => return JsValue::Undefined,
    };
    let mut acc = args.get(1).cloned().unwrap_or_else(|| {
        if arr.is_empty() {
            JsValue::Undefined
        } else {
            arr[0].clone()
        }
    });
    let start = if args.get(1).is_some() { 0 } else { 1 };
    for (i, item) in arr.iter().enumerate().skip(start) {
        acc = vm.call_function(&func, vec![acc, item.clone(), JsValue::Number(i as f64)]);
    }
    acc
}

fn native_array_flat(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let mut result = Vec::new();
    for item in arr {
        match item {
            JsValue::Array(inner) => result.extend(inner),
            other => result.push(other),
        }
    }
    JsValue::Array(result)
}

fn native_array_fill(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let mut arr = get_this_array(vm);
    let val = args.first().cloned().unwrap_or(JsValue::Undefined);
    let start = args.get(1).map(|a| a.to_number() as usize).unwrap_or(0);
    let end = args
        .get(2)
        .map(|a| a.to_number() as usize)
        .unwrap_or(arr.len());
    for i in start..end.min(arr.len()) {
        arr[i] = val.clone();
    }
    let result = JsValue::Array(arr.clone());
    vm.this_value = JsValue::Array(arr);
    result
}

fn native_array_to_string(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    let arr = get_this_array(vm);
    let parts: Vec<String> = arr.iter().map(|v| v.to_string_val()).collect();
    JsValue::Str(parts.join(","))
}

fn native_array_is_array(_vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    JsValue::Bool(matches!(args.first(), Some(JsValue::Array(_))))
}

fn native_array_from(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    match args.first() {
        Some(JsValue::Array(arr)) => JsValue::Array(arr.clone()),
        Some(JsValue::Str(s)) => {
            JsValue::Array(s.chars().map(|c| JsValue::Str(c.to_string())).collect())
        }
        Some(JsValue::Object(id)) => {
            // Check for length property (array-like)
            if let Some(JsValue::Number(len)) = vm.heap[*id].properties.get("length") {
                let len = *len as usize;
                let mut arr = Vec::with_capacity(len);
                for i in 0..len {
                    let val = vm.heap[*id]
                        .properties
                        .get(&i.to_string())
                        .cloned()
                        .unwrap_or(JsValue::Undefined);
                    arr.push(val);
                }
                JsValue::Array(arr)
            } else {
                JsValue::Array(vec![])
            }
        }
        _ => JsValue::Array(vec![]),
    }
}

// ─── JSON native functions ──────────────────────────────────────────────

fn json_to_jsvalue_vm(vm: &mut Vm, v: &serde_json::Value) -> JsValue {
    match v {
        serde_json::Value::Null => JsValue::Null,
        serde_json::Value::Bool(b) => JsValue::Bool(*b),
        serde_json::Value::Number(n) => JsValue::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => JsValue::Str(s.clone()),
        serde_json::Value::Array(arr) => {
            JsValue::Array(arr.iter().map(|v| json_to_jsvalue_vm(vm, v)).collect())
        }
        serde_json::Value::Object(map) => {
            let obj_id = vm.alloc_object();
            for (key, val) in map {
                let jv = json_to_jsvalue_vm(vm, val);
                vm.heap[obj_id].properties.insert(key.clone(), jv);
            }
            JsValue::Object(obj_id)
        }
    }
}

fn native_json_parse(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let s = match args.first() {
        Some(JsValue::Str(s)) => s.clone(),
        Some(other) => other.to_string_val(),
        None => return JsValue::Undefined,
    };
    match serde_json::from_str::<serde_json::Value>(&s) {
        Ok(val) => json_to_jsvalue_vm(vm, &val),
        Err(_) => JsValue::Undefined, // should throw SyntaxError, but we'll be lenient
    }
}

fn jsvalue_to_json(vm: &Vm, val: &JsValue) -> serde_json::Value {
    match val {
        JsValue::Undefined => serde_json::Value::Null,
        JsValue::Null => serde_json::Value::Null,
        JsValue::Bool(b) => serde_json::Value::Bool(*b),
        JsValue::Number(n) => {
            if n.is_finite() {
                serde_json::Value::Number(
                    serde_json::Number::from_f64(*n).unwrap_or(serde_json::Number::from(0)),
                )
            } else {
                serde_json::Value::Null
            }
        }
        JsValue::Str(s) => serde_json::Value::String(s.clone()),
        JsValue::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(|v| jsvalue_to_json(vm, v)).collect())
        }
        JsValue::Object(id) => {
            let mut map = serde_json::Map::new();
            if let Some(obj) = vm.heap.get(*id) {
                for (k, v) in &obj.properties {
                    if !k.starts_with("__") {
                        // skip internal properties
                        map.insert(k.clone(), jsvalue_to_json(vm, v));
                    }
                }
            }
            serde_json::Value::Object(map)
        }
        JsValue::Function(_) | JsValue::NativeFunction(_) => serde_json::Value::Null,
    }
}

fn native_json_stringify(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let val = args.first().cloned().unwrap_or(JsValue::Undefined);
    let json_val = jsvalue_to_json(vm, &val);
    match serde_json::to_string(&json_val) {
        Ok(s) => JsValue::Str(s),
        Err(_) => JsValue::Undefined,
    }
}

// ─── Object native functions ────────────────────────────────────────────

fn native_object_keys(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    match args.first() {
        Some(JsValue::Object(id)) => {
            if let Some(obj) = vm.heap.get(*id) {
                let keys: Vec<JsValue> = obj
                    .properties
                    .keys()
                    .filter(|k| !k.starts_with("__"))
                    .map(|k| JsValue::Str(k.clone()))
                    .collect();
                JsValue::Array(keys)
            } else {
                JsValue::Array(vec![])
            }
        }
        _ => JsValue::Array(vec![]),
    }
}

fn native_object_values(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    match args.first() {
        Some(JsValue::Object(id)) => {
            if let Some(obj) = vm.heap.get(*id) {
                let vals: Vec<JsValue> = obj
                    .properties
                    .iter()
                    .filter(|(k, _)| !k.starts_with("__"))
                    .map(|(_, v)| v.clone())
                    .collect();
                JsValue::Array(vals)
            } else {
                JsValue::Array(vec![])
            }
        }
        _ => JsValue::Array(vec![]),
    }
}

fn native_object_entries(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    match args.first() {
        Some(JsValue::Object(id)) => {
            if let Some(obj) = vm.heap.get(*id) {
                let entries: Vec<JsValue> = obj
                    .properties
                    .iter()
                    .filter(|(k, _)| !k.starts_with("__"))
                    .map(|(k, v)| JsValue::Array(vec![JsValue::Str(k.clone()), v.clone()]))
                    .collect();
                JsValue::Array(entries)
            } else {
                JsValue::Array(vec![])
            }
        }
        _ => JsValue::Array(vec![]),
    }
}

fn native_object_assign(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let target_id = match args.first() {
        Some(JsValue::Object(id)) => *id,
        _ => return args.first().cloned().unwrap_or(JsValue::Undefined),
    };
    for source in args.iter().skip(1) {
        if let JsValue::Object(src_id) = source {
            if let Some(src_obj) = vm.heap.get(*src_id).cloned() {
                for (k, v) in src_obj.properties {
                    if !k.starts_with("__") {
                        vm.heap[target_id].properties.insert(k, v);
                    }
                }
            }
        }
    }
    JsValue::Object(target_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(source: &str) -> (JsValue, ConsoleOutput) {
        let mut vm = Vm::new();
        let _ = vm.eval(source);
        let output = vm.console_output.clone();
        // Get last value from globals or stack
        (JsValue::Undefined, output)
    }

    fn eval_global(source: &str, name: &str) -> JsValue {
        let mut vm = Vm::new();
        let _ = vm.eval(source).unwrap();
        vm.globals.get(name).cloned().unwrap_or(JsValue::Undefined)
    }

    #[test]
    fn test_simple_arithmetic() {
        let val = eval_global("var x = 2 + 3;", "x");
        if let JsValue::Number(n) = val {
            assert_eq!(n, 5.0);
        } else {
            panic!("Expected number, got {:?}", val);
        }
    }

    #[test]
    fn test_string_concat() {
        let val = eval_global("var x = 'hello' + ' ' + 'world';", "x");
        if let JsValue::Str(s) = val {
            assert_eq!(s, "hello world");
        } else {
            panic!("Expected string, got {:?}", val);
        }
    }

    #[test]
    fn test_console_log() {
        let (_, output) = eval("console.log('hello', 42);");
        assert_eq!(output.lines.len(), 1);
        assert_eq!(output.lines[0], "hello 42");
    }

    #[test]
    fn test_function_call() {
        let val = eval_global(
            "function add(a, b) { return a + b; } var x = add(3, 4);",
            "x",
        );
        if let JsValue::Number(n) = val {
            assert_eq!(n, 7.0);
        } else {
            panic!("Expected number, got {:?}", val);
        }
    }

    #[test]
    fn test_if_else() {
        let val = eval_global(
            "var x = 10; var y; if (x > 5) { y = 'big'; } else { y = 'small'; }",
            "y",
        );
        if let JsValue::Str(s) = val {
            assert_eq!(s, "big");
        } else {
            panic!("Expected string, got {:?}", val);
        }
    }

    #[test]
    fn test_while_loop() {
        let val = eval_global(
            "var i = 0; var sum = 0; while (i < 5) { sum = sum + i; i = i + 1; }",
            "sum",
        );
        if let JsValue::Number(n) = val {
            assert_eq!(n, 10.0); // 0+1+2+3+4
        } else {
            panic!("Expected number, got {:?}", val);
        }
    }

    #[test]
    fn test_for_loop() {
        let val = eval_global(
            "var sum = 0; for (var i = 1; i <= 5; i = i + 1) { sum = sum + i; }",
            "sum",
        );
        if let JsValue::Number(n) = val {
            assert_eq!(n, 15.0); // 1+2+3+4+5
        } else {
            panic!("Expected number, got {:?}", val);
        }
    }

    #[test]
    fn test_nested_function() {
        let val = eval_global(
            "function outer() { function inner(x) { return x * 2; } return inner(21); } var x = outer();",
            "x",
        );
        if let JsValue::Number(n) = val {
            assert_eq!(n, 42.0);
        } else {
            panic!("Expected number, got {:?}", val);
        }
    }

    #[test]
    fn test_comparison_ops() {
        let val = eval_global("var x = (3 === 3);", "x");
        assert!(matches!(val, JsValue::Bool(true)));

        let val = eval_global("var x = (3 !== 4);", "x");
        assert!(matches!(val, JsValue::Bool(true)));

        let val = eval_global("var x = (2 < 3);", "x");
        assert!(matches!(val, JsValue::Bool(true)));
    }

    #[test]
    fn test_logical_ops() {
        let val = eval_global("var x = true && false;", "x");
        assert!(matches!(val, JsValue::Bool(false)));

        let val = eval_global("var x = false || true;", "x");
        assert!(matches!(val, JsValue::Bool(true)));
    }

    #[test]
    fn test_array_literal() {
        let val = eval_global("var x = [1, 2, 3];", "x");
        if let JsValue::Array(arr) = val {
            assert_eq!(arr.len(), 3);
        } else {
            panic!("Expected array, got {:?}", val);
        }
    }

    #[test]
    fn test_object_literal() {
        let val = eval_global("var x = { a: 1, b: 2 };", "x");
        if let JsValue::Object(id) = val {
            // Object was created
            assert!(id < 100); // sanity check
        } else {
            panic!("Expected object, got {:?}", val);
        }
    }

    #[test]
    fn test_math_built_in() {
        let val = eval_global("var x = Math.floor(3.7);", "x");
        if let JsValue::Number(n) = val {
            assert_eq!(n, 3.0);
        } else {
            panic!("Expected number, got {:?}", val);
        }
    }

    #[test]
    fn test_typeof() {
        let val = eval_global("var x = typeof 42;", "x");
        assert!(matches!(val, JsValue::Str(ref s) if s == "number"));

        let val = eval_global("var x = typeof 'hi';", "x");
        assert!(matches!(val, JsValue::Str(ref s) if s == "string"));
    }

    #[test]
    fn test_ternary() {
        let val = eval_global("var x = true ? 'yes' : 'no';", "x");
        assert!(matches!(val, JsValue::Str(ref s) if s == "yes"));

        let val = eval_global("var x = false ? 'yes' : 'no';", "x");
        assert!(matches!(val, JsValue::Str(ref s) if s == "no"));
    }

    #[test]
    fn test_recursive_function() {
        let val = eval_global(
            "function fact(n) { if (n <= 1) { return 1; } return n * fact(n - 1); } var x = fact(5);",
            "x",
        );
        if let JsValue::Number(n) = val {
            assert_eq!(n, 120.0);
        } else {
            panic!("Expected number, got {:?}", val);
        }
    }
}
