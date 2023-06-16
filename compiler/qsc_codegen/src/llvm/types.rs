// Portions copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::module::AddrSpace;
use std::borrow::Borrow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::{self, Display};
use std::hash::Hash;
use std::ops::Deref;
use std::sync::Arc;

/// See [LLVM 14 docs on Type System](https://releases.llvm.org/14.0.0/docs/LangRef.html#type-system)
#[derive(PartialEq, Eq, Clone, Debug, Hash)]
#[allow(non_camel_case_types)]
pub enum Type {
    /// See [LLVM 14 docs on Void Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#void-type)
    Void,
    /// See [LLVM 14 docs on Integer Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#integer-type)
    Integer { bits: u32 },
    /// See [LLVM 14 docs on Pointer Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#pointer-type)
    Pointer {
        pointee_type: TypeRef,
        addr_space: AddrSpace,
    },
    /// See [LLVM 14 docs on Floating-Point Types](https://releases.llvm.org/14.0.0/docs/LangRef.html#floating-point-types)
    Fp(FPType),
    /// See [LLVM 14 docs on Function Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#function-type)
    Func {
        result_type: TypeRef,
        param_types: Vec<TypeRef>,
        is_var_arg: bool,
    },
    /// Vector types (along with integer, FP, pointer, X86_MMX, and X86_AMX types) are "first class types",
    /// which means they can be produced by instructions (see [LLVM 14 docs on First Class Types](https://releases.llvm.org/14.0.0/docs/LangRef.html#first-class-types)).
    /// See [LLVM 14 docs on Vector Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#vector-type)
    Vector {
        element_type: TypeRef,
        num_elements: u32,
        scalable: bool,
    },
    /// Struct and Array types (but not vector types) are "aggregate types" and cannot be produced by
    /// a single instruction (see [LLVM 14 docs on Aggregate Types](https://releases.llvm.org/14.0.0/docs/LangRef.html#aggregate-types)).
    /// See [LLVM 14 docs on Array Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#array-type)
    Array {
        element_type: TypeRef,
        num_elements: usize,
    },
    /// The `StructType` variant is used for a "literal" (i.e., anonymous) structure type.
    /// See [LLVM 14 docs on Structure Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#structure-type)
    Struct {
        element_types: Vec<TypeRef>,
        is_packed: bool,
    },
    /// Named structure types. Note that these may be self-referential (i.e., recursive).
    /// See [LLVM 14 docs on Structure Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#structure-type)
    /// To get the actual definition of a named structure type, use `module.types.named_struct_def()`.
    NamedStruct {
        /// Name of the struct type
        name: String, // llvm-hs-pure has Name rather than String
    },
    /// See [LLVM 14 docs on Metadata Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#metadata-type)
    Metadata,
    /// `LabelType` is the type of [`BasicBlock`](../struct.BasicBlock.html) labels.
    /// See [LLVM 14 docs on Label Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#label-type)
    Label,
    /// See [LLVM 14 docs on Token Type](https://releases.llvm.org/14.0.0/docs/LangRef.html#token-type)
    Token,
}

impl Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Type::Void => write!(f, "void"),
            Type::Integer { bits } => write!(f, "i{bits}"),
            Type::Pointer { pointee_type, .. } => write!(f, "{pointee_type}*"),
            Type::Fp(fpt) => write!(f, "{fpt}"),
            Type::Func {
                result_type,
                param_types,
                is_var_arg,
            } => {
                write!(f, "{result_type} (")?;
                for (i, param_ty) in param_types.iter().enumerate() {
                    if i == param_types.len() - 1 {
                        write!(f, "{param_ty}")?;
                    } else {
                        write!(f, "{param_ty}, ")?;
                    }
                }
                if *is_var_arg {
                    write!(f, ", ...")?;
                }
                write!(f, ")")?;
                Ok(())
            }
            Type::Vector {
                element_type,
                num_elements,
                scalable,
            } => {
                if *scalable {
                    write!(f, "<vscale x {num_elements} x {element_type}>")
                } else {
                    write!(f, "<{num_elements} x {element_type}>")
                }
            }
            Type::Array {
                element_type,
                num_elements,
            } => write!(f, "[{num_elements} x {element_type}]"),
            Type::Struct {
                element_types,
                is_packed,
            } => {
                if *is_packed {
                    write!(f, "<")?;
                }
                write!(f, "{{ ")?;
                for (i, element_ty) in element_types.iter().enumerate() {
                    if i == element_types.len() - 1 {
                        write!(f, "{element_ty}")?;
                    } else {
                        write!(f, "{element_ty}, ")?;
                    }
                }
                write!(f, " }}")?;
                if *is_packed {
                    write!(f, ">")?;
                }
                Ok(())
            }
            Type::NamedStruct { name } => write!(f, "%{name}"),
            Type::Metadata => write!(f, "metadata"),
            Type::Label => write!(f, "label"),
            Type::Token => write!(f, "token"),
        }
    }
}

/// See [LLVM 14 docs on Floating-Point Types](https://releases.llvm.org/14.0.0/docs/LangRef.html#floating-point-types)
#[derive(PartialEq, Eq, Clone, Copy, Debug, Hash)]
#[allow(non_camel_case_types)]
pub enum FPType {
    Half,
    BFloat,
    Single,
    Double,
    FP128,
    X86_FP80,
    PPC_FP128,
}

impl From<FPType> for Type {
    fn from(fpt: FPType) -> Type {
        Type::Fp(fpt)
    }
}

impl Display for FPType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FPType::Half => write!(f, "half"),
            FPType::BFloat => write!(f, "bfloat"),
            FPType::Single => write!(f, "float"),
            FPType::Double => write!(f, "double"),
            FPType::FP128 => write!(f, "fp128"),
            FPType::X86_FP80 => write!(f, "x86_fp80"),
            FPType::PPC_FP128 => write!(f, "ppc_fp128"),
        }
    }
}

/// A `TypeRef` is a reference to a [`Type`](enum.Type.html).
/// Most importantly, it implements `AsRef<Type>` and `Deref<Target = Type>`.
/// It also has a cheap `Clone` -- only the reference is cloned, not the
/// underlying `Type`.
//
// `Arc` is used rather than `Rc` so that `Module` can remain `Sync`.
// This is important because it allows multiple threads to simultaneously access
// a single (immutable) `Module`.
#[derive(PartialEq, Eq, Clone, Debug, Hash)]
pub struct TypeRef(Arc<Type>);

impl AsRef<Type> for TypeRef {
    fn as_ref(&self) -> &Type {
        self.0.as_ref()
    }
}

impl Deref for TypeRef {
    type Target = Type;

    fn deref(&self) -> &Type {
        &self.0
    }
}

impl Display for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", &self.0)
    }
}

impl TypeRef {
    /// For use only in this module: construct a `TypeRef` by consuming the given owned `Type`.
    /// External users should get `TypeRefs` only from the `Types` or `TypesBuilder` objects.
    fn new(ty: Type) -> Self {
        Self(Arc::new(ty))
    }
}

/// Holds a reference to all of the `Type`s used in the `Module`, and facilitates
/// lookups so you can get a `TypeRef` to the `Type` you want.
#[derive(Clone)]
pub struct Builder {
    /// `TypeRef` to `Type::VoidType`
    void_type: TypeRef,
    /// Map of integer size to `Type::IntegerType` of that size
    int_types: TypeCache<u32>,
    /// Map of (pointee type, address space) to the corresponding `Type::PointerType`
    pointer_types: TypeCache<(TypeRef, AddrSpace)>,
    /// Map of `FPType` to the corresponding `Type::FPType`
    fp_types: TypeCache<FPType>,
    /// Map of `(result_type, param_types, is_var_arg)` to the corresponding `Type::FunctionType`
    func_types: TypeCache<(TypeRef, Vec<TypeRef>, bool)>,
    /// Map of (element type, #elements, scalable) to the corresponding `Type::VectorType`
    vec_types: TypeCache<(TypeRef, usize, bool)>,
    /// Map of (element type, #elements) to the corresponding `Type::ArrayType`
    arr_types: TypeCache<(TypeRef, usize)>,
    /// Map of `(element_types, is_packed)` to the corresponding `Type::StructType`
    struct_types: TypeCache<(Vec<TypeRef>, bool)>,
    /// Map of struct name to the corresponding `Type::NamedStructType`
    named_struct_types: TypeCache<String>,
    /// Map of struct name to the corresponding `NamedStructDef`
    named_struct_defs: HashMap<String, NamedStructDef>,
    /// `TypeRef` to `Type::MetadataType`
    metadata_type: TypeRef,
    /// `TypeRef` to `Type::LabelType`
    label_type: TypeRef,
    /// `TypeRef` to `Type::TokenType`
    token_type: TypeRef,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for Builder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, def) in &self.named_struct_defs {
            writeln!(f, "%{name} = type {def}")?;
        }
        Ok(())
    }
}

impl Builder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            void_type: TypeRef::new(Type::Void),
            int_types: TypeCache::new(),
            pointer_types: TypeCache::new(),
            fp_types: TypeCache::new(),
            func_types: TypeCache::new(),
            vec_types: TypeCache::new(),
            arr_types: TypeCache::new(),
            struct_types: TypeCache::new(),
            named_struct_types: TypeCache::new(),
            named_struct_defs: HashMap::new(),
            metadata_type: TypeRef::new(Type::Metadata),
            label_type: TypeRef::new(Type::Label),
            token_type: TypeRef::new(Type::Token),
        }
    }

    /// Get the void type
    #[must_use]
    pub fn void(&self) -> TypeRef {
        self.void_type.clone()
    }

    /// Get the opaque Qubit type
    #[must_use]
    pub fn qubit(&mut self) -> TypeRef {
        let qubit_ty = self.named_struct("Qubit".to_string());
        self.pointer_to(qubit_ty)
    }

    /// Get the opaque Result type
    #[must_use]
    pub fn result(&mut self) -> TypeRef {
        let result_ty = self.named_struct("Result".to_string());
        self.pointer_to(result_ty)
    }

    /// Get the integer type of the specified size (in bits)
    pub fn int(&mut self, bits: u32) -> TypeRef {
        self.int_types
            .lookup_or_insert(bits, || Type::Integer { bits })
    }

    /// Get the boolean type (`i1`)
    pub fn bool(&mut self) -> TypeRef {
        self.int(1)
    }

    /// Get the 8-bit integer type
    pub fn i8(&mut self) -> TypeRef {
        self.int(8)
    }

    /// Get the 16-bit integer type
    pub fn i16(&mut self) -> TypeRef {
        self.int(16)
    }

    /// Get the 32-bit integer type
    pub fn i32(&mut self) -> TypeRef {
        self.int(32)
    }

    /// Get the 64-bit integer type
    pub fn i64(&mut self) -> TypeRef {
        self.int(64)
    }

    /// Get a pointer type in the default address space (`0`)
    pub fn pointer_to(&mut self, pointee_type: TypeRef) -> TypeRef {
        self.pointer_in_addr_space(pointee_type, 0) // default to address space 0
    }

    /// Get a pointer in the specified address space
    pub fn pointer_in_addr_space(
        &mut self,
        pointee_type: TypeRef,
        addr_space: AddrSpace,
    ) -> TypeRef {
        self.pointer_types
            .lookup_or_insert((pointee_type.clone(), addr_space), || Type::Pointer {
                pointee_type,
                addr_space,
            })
    }

    /// Get a floating-point type
    pub fn fp(&mut self, fpt: FPType) -> TypeRef {
        self.fp_types.lookup_or_insert(fpt, || Type::Fp(fpt))
    }

    /// Get the single-precision floating-point type
    pub fn single(&mut self) -> TypeRef {
        self.fp(FPType::Single)
    }

    /// Get the double-precision floating-point type
    pub fn double(&mut self) -> TypeRef {
        self.fp(FPType::Double)
    }

    /// Get a function type
    pub fn func_type(
        &mut self,
        result_type: TypeRef,
        param_types: Vec<TypeRef>,
        is_var_arg: bool,
    ) -> TypeRef {
        self.func_types.lookup_or_insert(
            (result_type.clone(), param_types.clone(), is_var_arg),
            || Type::Func {
                result_type,
                param_types,
                is_var_arg,
            },
        )
    }

    /// Get a vector type
    pub fn vector_of(
        &mut self,
        element_type: TypeRef,
        num_elements: u32,
        scalable: bool,
    ) -> TypeRef {
        self.vec_types.lookup_or_insert(
            (element_type.clone(), num_elements as usize, scalable),
            || Type::Vector {
                element_type,
                num_elements,
                scalable,
            },
        )
    }

    /// Get an array type
    pub fn array_of(&mut self, element_type: TypeRef, num_elements: usize) -> TypeRef {
        self.arr_types
            .lookup_or_insert((element_type.clone(), num_elements), || Type::Array {
                element_type,
                num_elements,
            })
    }

    /// Get a struct type
    pub fn struct_of(&mut self, element_types: Vec<TypeRef>, is_packed: bool) -> TypeRef {
        self.struct_types
            .lookup_or_insert((element_types.clone(), is_packed), || Type::Struct {
                element_types,
                is_packed,
            })
    }

    /// Get the `TypeRef` for the struct with the given name.
    ///
    /// Note that this gives a `NamedStructType`.
    /// To get the actual _definition_ of a named struct (the `NamedStructDef`),
    /// use `named_struct_def()`.
    pub fn named_struct(&mut self, name: String) -> TypeRef {
        self.named_struct_types
            .lookup_or_insert(name.clone(), || Type::NamedStruct { name })
    }

    /// Get the `NamedStructDef` for the struct with the given `name`.
    ///
    /// Panics if no definition has been added for that struct name.
    ///
    /// Note that this gives a `NamedStructDef`.
    /// To get the `NamedStructType` for a `name`, use `named_struct()`.
    #[must_use]
    pub fn named_struct_def(&self, name: &str) -> &NamedStructDef {
        self.named_struct_defs
            .get(name)
            .expect("Named struct has not been defined")
    }

    /// Add the given `NamedStructDef` as the definition of the struct with the given `name`.
    ///
    /// # Panics
    /// This function panics if that name already had a definition.
    pub fn add_named_struct_def(&mut self, name: String, def: NamedStructDef) {
        match self.named_struct_defs.entry(name) {
            Entry::Occupied(_) => {
                panic!("Trying to redefine named struct");
            }
            Entry::Vacant(ventry) => {
                ventry.insert(def);
            }
        }
    }

    /// Get the metadata type
    #[must_use]
    pub fn metadata_type(&self) -> TypeRef {
        self.metadata_type.clone()
    }

    /// Get the label type
    #[must_use]
    pub fn label_type(&self) -> TypeRef {
        self.label_type.clone()
    }

    /// Get the token type
    #[must_use]
    pub fn token_type(&self) -> TypeRef {
        self.token_type.clone()
    }
}

#[derive(Clone, Debug, Hash)]
pub enum NamedStructDef {
    /// An opaque struct type; see [LLVM 14 docs on Opaque Structure Types](https://releases.llvm.org/14.0.0/docs/LangRef.html#t-opaque).
    Opaque,
    /// A struct type with a definition. The `TypeRef` here is guaranteed to be to a `StructType` variant.
    Defined(TypeRef),
}

impl Display for NamedStructDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NamedStructDef::Opaque => write!(f, "opaque")?,
            NamedStructDef::Defined(ty) => write!(f, "{ty}")?,
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct TypeCache<K: Eq + Hash + Clone> {
    map: HashMap<K, TypeRef>,
}

#[allow(dead_code)]
impl<K: Eq + Hash + Clone> TypeCache<K> {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// Get a `TypeRef` to the `Type` with the given key,
    /// or `None` if the `Type` is not present.
    fn lookup<Q: ?Sized>(&self, key: &Q) -> Option<TypeRef>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.map.get(key).cloned()
    }

    /// Get a `TypeRef` to the `Type` with the given key.
    /// The `if_missing` function or closure will be called to create that `Type`
    /// if it hasn't been created yet.
    fn lookup_or_insert(&mut self, key: K, if_missing: impl FnOnce() -> Type) -> TypeRef {
        self.map
            .entry(key)
            .or_insert_with(|| TypeRef::new(if_missing()))
            .clone()
    }

    /// Is a `Type` for the given key currently in the cache?
    fn contains_key(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }
}