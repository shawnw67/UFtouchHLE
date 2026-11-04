/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Handling of Objective-C classes and metaclasses.
//!
//! Note that metaclasses are just a special case of classes.
//!
//! Resources:
//! - [[objc explain]: Classes and metaclasses](http://www.sealiesoftware.com/blog/archive/2009/04/14/objc_explain_Classes_and_metaclasses.html), especially [the PDF diagram](http://www.sealiesoftware.com/blog/class%20diagram.pdf)

use super::{
    id, ivar_list_t, method_list_t, nil, objc_object, AnyHostObject, HostIMP, HostObject, ObjC,
    IMP, SEL,
};
use crate::libc::string::strdup;
use crate::mach_o::MachO;
use crate::mem::MutVoidPtr;
use crate::mem::{guest_size_of, ConstPtr, ConstVoidPtr, GuestUSize, Mem, MutPtr, Ptr, SafeRead};
use crate::Environment;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};

/// Generic pointer to an Objective-C class or metaclass.
///
/// The name is standard Objective-C.
///
/// Apple's runtime has a `objc_classes` definition similar to `objc_object`.
/// We could do the same thing here, but it doesn't seem worth it, as we can't
/// get the same unidirectional type safety.
pub type Class = id;

/// Our internal representation of a class, e.g. this is where `objc_msgSend`
/// will look up method implementations.
///
/// Note: `superclass` can be `nil`!
pub(super) struct ClassHostObject {
    pub(super) name: String,
    pub(super) is_metaclass: bool,
    pub(super) superclass: Class,
    pub(super) methods: crate::fast_hash::FastHashMap<SEL, IMP>,
    pub(super) guest_method_signatures: crate::fast_hash::FastHashMap<SEL, ConstPtr<u8>>,
    /// Maps ivar name to a tuple of an offset (as pointer) and an alignment.
    /// (Alignment is used during ivar reconciliation.)
    pub(super) ivars: HashMap<String, (ConstPtr<GuestUSize>, u32)>,
    /// Maps ivar name to its `ivar_t` in guest memory, which is the `Ivar`
    /// handle of the runtime API.
    pub(super) ivar_metadata: HashMap<String, ConstVoidPtr>,
    /// Maps a declared `@property` name to its `property_t` in guest memory.
    pub(super) properties: HashMap<String, ConstVoidPtr>,
    /// Protocols directly adopted by the class.
    pub(super) protocols: Vec<id>,
    /// Offset into the allocated memory for the object where the ivars of
    /// instances of this class or metaclass (respectively: normal objects or
    /// classes) should live. This is always >= the value in the superclass.
    pub(super) instance_start: GuestUSize,
    /// Size of the allocated memory for instances of this class or metaclass.
    /// This is always >= the value in the superclass.
    pub(super) instance_size: GuestUSize,
    /// Checks if +initialize has been called yet.
    pub(super) is_initialized: InitializationStatus,
}
impl HostObject for ClassHostObject {}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum InitializationStatus {
    NotInitialized,
    Initializing,
    Initialized,
}

/// Placeholder object for classes and metaclasses referenced by the app that
/// we don't have an implementation for.
///
/// This lets us delay errors about missing implementations until the first
/// time the app actually uses them (e.g. when a message is sent).
pub(super) struct UnimplementedClass {
    pub(super) name: String,
    pub(super) is_metaclass: bool,
}
impl HostObject for UnimplementedClass {}

/// Substitute object for classes and metaclasses from the guest app that we do
/// not want to support (see [substitute_classes]).
///
/// Messages sent to this class will behave as if messaging [nil].
pub(super) struct FakeClass {
    pub(super) name: String,
    pub(super) is_metaclass: bool,
}
impl HostObject for FakeClass {}

/// The layout of a class in an app binary.
///
/// The name, field names and field layout are based on what Ghidra outputs.
#[repr(C, packed)]
#[allow(dead_code)]
struct class_t {
    isa: Class, // note that this matches objc_object
    superclass: Class,
    _cache: ConstVoidPtr,
    _vtable: ConstVoidPtr,
    data: ConstPtr<class_rw_t>,
}
unsafe impl SafeRead for class_t {}

/// The layout of the main class data in an app binary.
///
/// The name, field names and field layout are based on what Ghidra's output.
#[repr(C, packed)]
#[allow(dead_code)]
struct class_rw_t {
    _flags: u32,
    instance_start: GuestUSize,
    instance_size: GuestUSize,
    _reserved: u32,
    name: ConstPtr<u8>,
    base_methods: ConstPtr<method_list_t>,
    _base_protocols: ConstVoidPtr, // protocol list (TODO)
    ivars: ConstPtr<ivar_list_t>,
    _weak_ivar_layout: u32,
    _base_properties: ConstVoidPtr, // property list (TODO)
}
unsafe impl SafeRead for class_rw_t {}

/// The layout of a category in an app binary.
///
/// The name, field names and field layout are based on what Ghidra outputs.
#[repr(C, packed)]
struct category_t {
    name: ConstPtr<u8>,
    class: Class,
    instance_methods: ConstPtr<method_list_t>,
    class_methods: ConstPtr<method_list_t>,
    _protocols: ConstVoidPtr,     // protocol list (TODO)
    _property_list: ConstVoidPtr, // property list (TODO)
}
unsafe impl SafeRead for category_t {}

#[repr(C, packed)]
pub struct objc_property {
    // TODO: define fields?
    _pad: u8,
}
unsafe impl SafeRead for objc_property {}

/// An opaque type that represents an Objective-C declared property.
#[allow(non_camel_case_types)]
type objc_property_t = MutPtr<objc_property>;

/// A template for a class defined with [objc_classes].
///
/// Host implementations of libraries can use these to expose classes to the
/// application. The runtime will create the actual class ([ClassHostObject]
/// etc) from the template on-demand. See also [ClassExports].
pub struct ClassTemplate {
    pub name: &'static str,
    pub superclass: Option<&'static str>,
    pub class_methods: &'static [(&'static str, &'static dyn HostIMP)],
    pub instance_methods: &'static [(&'static str, &'static dyn HostIMP)],
}

/// Type for lists of classes exported by host implementations of frameworks.
///
/// Each module that wants to expose functions to guest code should export a
/// constant using this type. See [objc_classes] for an example.
///
/// All the constants like this can then be collected into a
/// [crate::dyld::HostDylib].
///
/// The strings are the class names.
///
/// See also [crate::dyld::ConstantExports] and [crate::dyld::FunctionExports].
pub type ClassExports = &'static [(&'static str, ClassTemplate)];

#[doc(hidden)]
#[macro_export]
macro_rules! _objc_superclass {
    (: $name:ident) => {
        Some(stringify!($name))
    };
    () => {
        None
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! _objc_method {
    (
        $env:ident,
        $this:ident,
        $_cmd:ident,
        $cmd_name:ident,
        $retty:ty,
        $block:block
        $(, $ty:ty, $arg:ident)*
        $(, ...$va_arg:ident: $va_type:ty)?
    ) => {
        // The closure must be explicitly casted because a bare closure defaults
        // to a different type than a pure fn pointer, which is the type that
        // HostIMP and CallFromGuest are implemented on.
        &((|
            #[allow(unused_variables)]
            $env: &mut $crate::Environment,
            #[allow(unused_variables)]
            $this: $crate::objc::id,
            #[allow(unused_variables)]
            $_cmd: $crate::objc::SEL,
            $($arg: $ty,)*
            $(#[allow(unused_mut)] mut $va_arg: $va_type,)?
        | -> $retty {
            const _OBJC_CURRENT_SELECTOR: &str = stringify!($cmd_name);
            $block
        }) as fn(
            &mut $crate::Environment,
            $crate::objc::id,
            $crate::objc::SEL,
            $($ty,)*
            $($va_type,)?
        ) -> $retty)
    }
}

/// Macro for creating a list of [ClassTemplate]s (i.e. [ClassExports]).
/// It imitates the Objective-C class definition syntax.
///
/// ```ignore
/// pub const CLASSES: ClassExports = objc_classes! {
/// (env, this, _cmd); // Specify names of HostIMP implicit parameters.
///                    // The second one should be `self` to match Objective-C,
///                    // but that's reserved in Rust, hence `this`.
///
/// @implementation MyClass: NSObject
///
/// + (id)foo {
///     // ...
/// }
///
/// - (id)barWithQux:(u32)qux {
///     // ...
/// }
///
/// - (id)barWithVaArgs:(u32)qux, ...dots {
///     // ...
/// }
///
/// @end
/// };
/// ```
///
/// will desugar to approximately:
///
/// ```ignore
/// pub const CLASSES: ClassExports = &[
///     ("MyClass", ClassTemplate {
///         name: "MyClass",
///         superclass: Some("NSObject"),
///         class_methods: &[
///             ("foo", &(|env: &mut Environment, this: id, _cmd: SEL| -> id {
///                 // ...
///             } as fn(&mut Environment, id, SEL) -> id)),
///         ],
///         instance_methods: &[
///             ("barWithQux:", &(|
///                 env: &mut Environment,
///                 this: id,
///                 _cmd: SEL,
///                 qux: u32
///             | -> id {
///                 // ...
///             } as &fn(&mut Environment, id, SEL, u32) -> id)),
///             ("barWithVaArgs:", &(|
///                 env: &mut Environment,
///                 this: id,
///                 _cmd: SEL,
///                 qux: u32,
///                 va_args: DotDotDot
///             | -> id {
///                 // ...
///             } as &fn(&mut Environment, id, SEL, u32, DotDotDot) -> id)),
///         ],
///     })
/// ];
/// ```
///
/// Note that the instance methods must be preceded by the class methods.
#[macro_export] // documentation comment links are annoying without this
macro_rules! objc_classes {
    {
        // Rust's macro hygiene prevents the macro's own names for these
        // parameters being visible, so we have to get names supplied by the
        // macro user.
        ($env:ident, $this:ident, $_cmd:ident);
        $(
            @implementation $class_name:ident $(: $superclass_name:ident)?

            $( + ($cm_type:ty) $cm_name:ident $(:($cm_type1:ty) $cm_arg1:ident $($($cm_namen:ident)?:($cm_typen:ty) $cm_argn:ident)*)?
                              $(, ...$cm_va_arg:ident)?
                 $cm_block:block )*

            $( - ($im_type:ty) $im_name:ident $(:($im_type1:ty) $im_arg1:ident $($($im_namen:ident)?:($im_typen:ty) $im_argn:ident)*)?
                              $(, ...$im_va_arg:ident)?
                 $im_block:block )*

            @end
        )+
    } => {
        &[
            $({
                // This constant is for `msg_super!`, which needs to know which
                // class it is has been written within (not the same as the
                // runtime type of `this`, which could be a subclass). This is
                // a constant instead of a let binding because that escapes
                // Rust's macro hygiene.
                const _OBJC_CURRENT_CLASS: &str = stringify!($class_name);

                (_OBJC_CURRENT_CLASS, $crate::objc::ClassTemplate {
                    name: _OBJC_CURRENT_CLASS,
                    superclass: $crate::_objc_superclass!($(: $superclass_name)?),
                    class_methods: &[
                        $(
                            (
                                $crate::objc::selector!(
                                    $(($cm_type1);)?
                                    $cm_name
                                    $($(, $($cm_namen)?)*)?
                                ),
                                $crate::_objc_method!(
                                    $env,
                                    $this,
                                    $_cmd,
                                    $cm_name,
                                    $cm_type,
                                    { $cm_block }
                                    $(, $cm_type1, $cm_arg1 $(, $cm_typen, $cm_argn)*)?
                                    $(, ...$cm_va_arg: $crate::abi::DotDotDot)?
                                )
                            )
                        ),*
                    ],
                    instance_methods: &[
                        $(
                            (
                                $crate::objc::selector!(
                                    $(($im_type1);)?
                                    $im_name
                                    $($(, $($im_namen)?)*)?
                                ),
                                $crate::_objc_method!(
                                    $env,
                                    $this,
                                    $_cmd,
                                    $im_name,
                                    $im_type,
                                    { $im_block }
                                    $(, $im_type1, $im_arg1 $(, $im_typen, $im_argn)*)?
                                    $(, ...$im_va_arg: $crate::abi::DotDotDot)?
                                )
                            )
                        ),*
                    ],
                })
            }),+
        ]
    }
}
pub use crate::objc_classes; // #[macro_export] is weird...

impl ClassHostObject {
    fn from_template(
        template: &ClassTemplate,
        is_metaclass: bool,
        superclass: Class,
        objc: &ObjC,
    ) -> Self {
        // For our host implementations we store all data in host objects, so
        // there are no ivars and the size is always just the isa pointer.
        // This is true for both classes and normal objects.
        let size = guest_size_of::<objc_object>();
        ClassHostObject {
            name: template.name.to_string(),
            is_metaclass,
            superclass,
            methods: HashMap::from_iter(
                (if is_metaclass {
                    template.class_methods
                } else {
                    template.instance_methods
                })
                .iter()
                .map(|&(name, host_imp)| {
                    // The selector should already have been registered by
                    // [ObjC::register_host_selectors], so we can panic
                    // if it hasn't been.
                    (objc.selectors[name], IMP::Host(host_imp))
                }),
            ),
            guest_method_signatures: HashMap::default(),
            // maybe this should be 0 for NSObject? does it matter?
            instance_start: size,
            instance_size: size,
            ivars: HashMap::default(),
            ivar_metadata: HashMap::default(),
            properties: HashMap::default(),
            protocols: Vec::new(),
            is_initialized: InitializationStatus::NotInitialized,
        }
    }

    fn from_bin(class: Class, is_metaclass: bool, mem: &Mem, objc: &mut ObjC) -> Self {
        let class_t {
            superclass, data, ..
        } = mem.read(class.cast());
        let class_rw_t {
            instance_start,
            instance_size,
            name,
            base_methods,
            _base_protocols,
            _base_properties,
            ivars,
            ..
        } = mem.read(data);

        let name = mem.cstr_at_utf8(name).unwrap().to_string();

        let mut host_object = ClassHostObject {
            name,
            is_metaclass,
            superclass,
            methods: Default::default(),
            guest_method_signatures: Default::default(),
            instance_start,
            instance_size,
            ivars: HashMap::new(),
            ivar_metadata: HashMap::new(),
            properties: HashMap::new(),
            protocols: protocol_list_entries(mem, _base_protocols),
            is_initialized: InitializationStatus::NotInitialized,
        };

        if !base_methods.is_null() {
            host_object.add_methods_from_bin(base_methods, mem, objc);
        }

        if !ivars.is_null() {
            host_object.add_ivars_from_bin(ivars, mem);
        }

        if !_base_properties.is_null() {
            host_object.add_properties_from_bin(_base_properties, mem);
        }

        host_object
    }

    // See methods.rs for binary method parsing
}

/// Decide whether a certain class/metaclass pair from the guest app should use
/// fake class host objects and return the substitutions if so.
///
/// This function is called when registering classes from the guest app. It
/// detects certain problematic classes that are, for example, too complex for
/// touchHLE to currently support, but which can be easily replaced with simple
/// fakes.
fn substitute_classes(
    mem: &Mem,
    class: Class,
    metaclass: Class,
) -> Option<(Box<FakeClass>, Box<FakeClass>)> {
    let class_t { data, .. } = mem.read(class.cast());
    let class_rw_t { name, .. } = mem.read(data);
    let name = mem.cstr_at_utf8(name).unwrap();

    // Currently the only thing we try to substitute: classes that seem to be
    // from various third-party advertising or social network SDKs.
    // Naturally it makes a lot of use of UIKit and networking in ways we
    // don't support yet. This isn't "ad blocking" because ads no longer work
    // on real devices anyway :)
    if !(name.starts_with("AdMob")
        || name.starts_with("AdWhirl")
        || name.starts_with("AltAds")
        || name.starts_with("FB") // Facebook
        || name.starts_with("Flurry")
        // Com2uS Hub (Inotia 2 and other Com2uS games), whose servers no
        // longer exist and which needs UITableView.
        || name.starts_with("Hub")
        || name.starts_with("Mobclix")
        || name.starts_with("OpenFeint")
        || name.starts_with("Tapjoy"))
    {
        return None;
    }

    {
        let class_t { data, .. } = mem.read(metaclass.cast());
        let class_rw_t {
            name: metaclass_name,
            ..
        } = mem.read(data);
        let metaclass_name = mem.cstr_at_utf8(metaclass_name).unwrap();
        assert!(name == metaclass_name);
    }

    log!(
        "Note: substituting fake class for {} to improve compatibility",
        name
    );

    let class_host_object = Box::new(FakeClass {
        name: name.to_string(),
        is_metaclass: false,
    });
    let metaclass_host_object = Box::new(FakeClass {
        name: name.to_string(),
        is_metaclass: true,
    });
    Some((class_host_object, metaclass_host_object))
}

impl ObjC {
    /// Find the `Ivar` handle of an instance variable, searching superclasses.
    pub(crate) fn class_lookup_ivar(&self, mut class: Class, name: &str) -> Option<ConstVoidPtr> {
        while class != nil {
            let host = self.borrow::<ClassHostObject>(class);
            if let Some(&metadata) = host.ivar_metadata.get(name) {
                return Some(metadata);
            }
            class = host.superclass;
        }
        None
    }

    /// Find the pointer to the offset of the instance variable with an `Ivar`
    /// handle, searching superclasses.
    pub(crate) fn class_lookup_ivar_offset_by_metadata(
        &self,
        mut class: Class,
        metadata: ConstVoidPtr,
    ) -> Option<ConstPtr<GuestUSize>> {
        while class != nil {
            let host = self.borrow::<ClassHostObject>(class);
            for (name, &candidate) in &host.ivar_metadata {
                if candidate == metadata {
                    return host.ivars.get(name).map(|&(offset, _)| offset);
                }
            }
            class = host.superclass;
        }
        None
    }

    /// For use by host functions when the caller wants to handle the
    /// "class does not exist" case gracefully (e.g. UIClassSwapper falling
    /// back to the NIB's original class when the app-defined custom class is
    /// missing). Returns `Some(class)` if the class is already registered or
    /// a host-side template exists for it; otherwise returns `None` instead
    /// of panicking or installing a placeholder.
    pub fn try_get_known_class(&mut self, name: &str, mem: &mut Mem) -> Option<Class> {
        if let Some(class) = self.get_class(name, /* is_metaclass: */ false, mem) {
            return Some(class);
        }
        if Self::find_template(name).is_some() {
            return Some(self.link_class(name, /* is_metaclass: */ false, mem));
        }
        None
    }

    /// Iterator over all known classes and their names.
    pub fn all_classes(&self) -> impl Iterator<Item = (&String, &Class)> {
        self.classes.iter()
    }

    fn get_class(&self, name: &str, is_metaclass: bool, mem: &Mem) -> Option<Class> {
        let class = self.classes.get(name).copied()?;
        Some(if is_metaclass {
            Self::read_isa(class, mem)
        } else {
            class
        })
    }

    fn find_template(name: &str) -> Option<&'static ClassTemplate> {
        crate::dyld::search_host_dylibs(|dylib| dylib.class_exports, name)
            .map(|&(_name, ref template)| template)
    }

    /// For use by [crate::dyld]: get the class or metaclass referenced by an
    /// external relocation in the app binary. If we don't have an
    /// implementation of the class, a placeholder is used.
    pub fn link_class(&mut self, name: &str, is_metaclass: bool, mem: &mut Mem) -> Class {
        self.link_class_inner(name, is_metaclass, mem, true)
    }

    /// For use by host functions: get a particular class. If we don't have an
    /// implementation of the class, panic.
    pub fn get_known_class(&mut self, name: &str, mem: &mut Mem) -> Class {
        self.link_class_inner(name, /* is_metaclass: */ false, mem, false)
    }

    fn link_class_inner(
        &mut self,
        name: &str,
        is_metaclass: bool,
        mem: &mut Mem,
        use_placeholder: bool,
    ) -> Class {
        // The class and metaclass must be created together and tracked
        // together, so even though this function only returns one pointer, it
        // must create both. The function must not care whether the metaclass
        // is requested first, or if the class is requested first.

        if let Some(class) = self.get_class(name, is_metaclass, mem) {
            return class;
        };

        let class_host_object: Box<dyn AnyHostObject>;
        let metaclass_host_object: Box<dyn AnyHostObject>;
        if let Some(template) = Self::find_template(name) {
            // We have a template (host implementation) for this class, use it.

            if let Some(superclass_name) = template.superclass {
                // Make sure we actually have a template for the superclass
                // before we try to link it, else we might get an unimplemented
                // class back and have weird problems down the line
                assert!(Self::find_template(superclass_name).is_some());
            }

            class_host_object = Box::new(ClassHostObject::from_template(
                template,
                /* is_metaclass: */ false,
                /* superclass: */
                template
                    .superclass
                    .map(|name| {
                        self.link_class(name, /* is_metaclass: */ false, mem)
                    })
                    .unwrap_or(nil),
                self,
            ));
            metaclass_host_object = Box::new(ClassHostObject::from_template(
                template,
                /* is_metaclass: */ true,
                /* superclass: */
                template
                    .superclass
                    .map(|name| {
                        self.link_class(name, /* is_metaclass: */ true, mem)
                    })
                    .unwrap_or(nil),
                self,
            ));
        } else {
            if !use_placeholder {
                panic!("Missing implementation for class {name}!");
            }

            // We don't have a real implementation for this class, use a
            // placeholder.

            class_host_object = Box::new(UnimplementedClass {
                name: name.to_string(),
                is_metaclass: false,
            });
            metaclass_host_object = Box::new(UnimplementedClass {
                name: name.to_string(),
                is_metaclass: true,
            });
        }

        // NSObject's metaclass is special: it is its own metaclass, and it's
        // the superclass of all other metaclasses.
        // (FIXME: this actually should apply to any class hiearchy root.)
        // This creates a chicken-and-egg problem so it has a special path.
        let metaclass = if name == "NSObject" {
            let metaclass = mem.alloc_and_write(objc_object { isa: nil });
            mem.write(metaclass, objc_object { isa: metaclass });
            self.register_static_object(metaclass, metaclass_host_object);
            metaclass
        } else {
            let isa = self.link_class("NSObject", /* is_metaclass: */ true, mem);
            self.alloc_static_object(isa, metaclass_host_object, mem)
        };

        let class = self.alloc_static_object(metaclass, class_host_object, mem);

        if name == "NSObject" {
            // NSObject's metaclass has its class as the superclass.
            self.borrow_mut::<ClassHostObject>(metaclass).superclass = class;
        }

        self.classes.insert(name.to_string(), class);

        if is_metaclass {
            metaclass
        } else {
            class
        }
    }

    /// For use by [crate::dyld]: register all the classes from the application
    /// binary.
    pub fn register_bin_classes(&mut self, bin: &MachO, mem: &mut Mem) {
        let Some(list) = bin.get_section("__objc_classlist") else {
            return;
        };

        assert!(list.size % 4 == 0);
        let base: ConstPtr<Class> = Ptr::from_bits(list.addr);
        for i in 0..(list.size / 4) {
            let class = mem.read(base + i);
            let metaclass = Self::read_isa(class, mem);

            let name = if let Some(fakes) = substitute_classes(mem, class, metaclass) {
                let (class_host_object, metaclass_host_object) = fakes;

                assert!(class_host_object.name == metaclass_host_object.name);
                let name = class_host_object.name.clone();

                self.register_static_object(class, class_host_object);
                self.register_static_object(metaclass, metaclass_host_object);
                name
            } else {
                let class_host_object = Box::new(ClassHostObject::from_bin(
                    class, /* is_metaclass: */ false, mem, self,
                ));
                let metaclass_host_object = Box::new(ClassHostObject::from_bin(
                    metaclass, /* is_metaclass: */ true, mem, self,
                ));

                assert!(class_host_object.name == metaclass_host_object.name);
                let name = class_host_object.name.clone();

                self.register_static_object(class, class_host_object);
                self.register_static_object(metaclass, metaclass_host_object);
                name
            };

            self.classes.insert(name.to_string(), class);
        }

        let mut queue = VecDeque::<Class>::new();
        let mut found_ns_object = false;
        // Second pass to build an inverted inheritance graph
        let mut inverted_inheritance = HashMap::<Class, Vec<Class>>::new();
        for (name, class) in self.classes.iter() {
            log_dbg!("class name {}", name);
            if name == "NSObject" {
                assert!(!found_ns_object);
                found_ns_object = true;
            }
            let class_host_object = self
                .get_host_object(*class)
                .unwrap()
                .as_any()
                .downcast_ref();
            let Some(ClassHostObject { superclass, .. }) = class_host_object else {
                // Skip FakeClass or UnimplementedClass
                continue;
            };

            if *superclass != nil {
                inverted_inheritance
                    .entry(*superclass)
                    .and_modify(|v| v.push(*class))
                    .or_insert(vec![*class]);
            } else {
                assert!(!queue.contains(class));
                queue.push_back(*class);
            }
        }
        // At least NSObject should be found as a root object
        assert!(found_ns_object);

        // Third pass to ensure no superclass has "grown into" any of its
        // subclasses.
        // (https://alwaysprocessing.blog/2023/03/12/objc-ivar-abi)

        // BFS starting from root(s) for ivar reconciliation
        //
        // It is required to be traversed in this order,
        // as one overgrown class potentially implies
        // reconciliation for _all of subclasses_
        while !queue.is_empty() {
            let next = queue.pop_front().unwrap();
            let (need, mut diff) = self.need_ivar_reconciliation(next);
            if need {
                let ClassHostObject {
                    name, superclass, ..
                } = self.borrow(next);
                log_dbg!(
                    "Class {} need ivar reconciliation with superclass {}!",
                    name,
                    &self.borrow::<ClassHostObject>(*superclass).name
                );

                let ClassHostObject {
                    ref mut instance_start,
                    ref mut instance_size,
                    ref mut ivars,
                    ..
                } = self.borrow_mut(next);

                if !ivars.is_empty() {
                    let mut max_alignment: u32 = 1;
                    for (offset, align) in ivars.values() {
                        if offset.is_null() {
                            // anonymous bitfield
                            continue;
                        }
                        max_alignment = max_alignment.max(*align);
                    }

                    let align_mask = max_alignment - 1;
                    diff = (diff + align_mask) & !align_mask;

                    for (offset, _) in ivars.values_mut() {
                        if offset.is_null() {
                            // anonymous bitfield
                            continue;
                        }

                        *offset = Ptr::from_bits((*offset).to_bits() + diff);
                    }
                }

                *instance_start += diff;
                *instance_size += diff;
            }
            if let Some(subclasses) = inverted_inheritance.get(&next) {
                queue.extend(subclasses);
            }
        }
    }

    fn need_ivar_reconciliation(&mut self, class: Class) -> (bool, u32) {
        let class_host_object = self.get_host_object(class).unwrap().as_any().downcast_ref();
        let Some(ClassHostObject {
            name,
            superclass,
            instance_start,
            instance_size,
            ..
        }) = class_host_object
        else {
            // The class might be a FakeClass or UnimplementedClass
            // In those cases we move on as they don't have ivars
            return (false, 0);
        };
        log_dbg!(
            "Checking need_ivar_reconciliation for {}, start {}, size {}",
            name,
            instance_start,
            instance_size
        );

        if *superclass == nil {
            return (false, 0);
        }

        let superclass_host_object = self
            .get_host_object(*superclass)
            .unwrap()
            .as_any()
            .downcast_ref();
        let Some(ClassHostObject {
            instance_size: superclass_instance_size,
            ..
        }) = superclass_host_object
        else {
            // Superclass could also be a FakeClass or UnimplementedClass
            return (false, 0);
        };

        let need = instance_start < superclass_instance_size;
        let diff = if need {
            superclass_instance_size - instance_start
        } else {
            0
        };
        (need, diff)
    }

    /// Dumps all classes available to the emulator in JSON to stdout.
    ///
    /// The JSON has the following form:
    /// ```json
    /// {
    ///     "object": "classes",
    ///     "classes": [
    ///         {
    ///             "name": ((name of class)),
    ///             "super": ((name of superclass, if available)),
    ///             "class_type": (("normal" | "unimplemented" | "fake"))
    ///         },
    ///         ...
    ///     ]
    /// }
    /// ```
    pub fn dump_classes(&self, file: &mut std::fs::File) -> Result<(), std::io::Error> {
        use std::io::Write;
        writeln!(file, "{{\n    \"object\": \"classes\",\n    \"classes\": [")?;
        for (i, (_, o)) in self.classes.iter().enumerate() {
            // Why doesn't json allow trailing commas...
            let comma = if i == self.classes.len() - 1 { "" } else { "," };

            let host_obj = self.get_host_object(*o).unwrap();

            if let Some(ClassHostObject {
                name,
                superclass: sup,
                ..
            }) = host_obj.as_any().downcast_ref()
            {
                if *sup == nil {
                    writeln!(
                        file,
                        "        {{ \"name\": \"{name}\", \"class_type\": \"normal\" }}{comma}"
                    )?;
                } else {
                    writeln!(
                        file,
                        "        {{ \"name\": \"{}\", \"super\": \"{}\", \"class_type\": \"normal\" }}{}",
                        name, self.get_class_name(*sup), comma
                    )?;
                }
            } else if let Some(UnimplementedClass { name, .. }) = host_obj.as_any().downcast_ref() {
                writeln!(
                    file,
                    "        {{ \"name\": \"{name}\", \"class_type\": \"unimplemented\" }}{comma}"
                )?;
            } else if let Some(FakeClass { name, .. }) = host_obj.as_any().downcast_ref() {
                writeln!(
                    file,
                    "        {{ \"name\": \"{name}\", \"class_type\": \"fake\" }}{comma}"
                )?;
            } else {
                panic!("Unrecognized class type!");
            }
        }
        writeln!(file, "    ]\n}}")
    }

    /// For use by [crate::dyld]: register all the categories from the
    /// application binary.
    pub fn register_bin_categories(&mut self, bin: &MachO, mem: &mut Mem) {
        let Some(list) = bin.get_section("__objc_catlist") else {
            return;
        };

        assert!(list.size % 4 == 0);
        let base: ConstPtr<ConstPtr<category_t>> = Ptr::from_bits(list.addr);
        for i in 0..(list.size / 4) {
            let cat_ptr = mem.read(base + i);
            let data = mem.read(cat_ptr);

            let name = mem.cstr_at_utf8(data.name).unwrap();
            let class = data.class;
            let metaclass = Self::read_isa(class, mem);

            for (class, methods) in [
                (class, data.instance_methods),
                (metaclass, data.class_methods),
            ] {
                if methods.is_null() {
                    continue;
                }

                let any = self.get_host_object(class).unwrap().as_any();
                if any.is::<FakeClass>() || any.is::<UnimplementedClass>() {
                    continue;
                }

                // Horrible workaround to avoid double-borrowing self:
                // temporarily replace the class object.
                let mut host_obj = std::mem::replace(
                    self.borrow_mut::<ClassHostObject>(class),
                    ClassHostObject {
                        name: Default::default(),
                        is_metaclass: Default::default(),
                        superclass: nil,
                        methods: Default::default(),
                        guest_method_signatures: Default::default(),
                        instance_start: Default::default(),
                        instance_size: Default::default(),
                        ivars: Default::default(),
                        ivar_metadata: Default::default(),
                        properties: Default::default(),
                        protocols: Default::default(),
                        is_initialized: InitializationStatus::NotInitialized,
                    },
                );
                log_dbg!(
                    "Adding {} methods from guest app category \"{}\" {:?} to {} \"{}\" {:?}",
                    if host_obj.is_metaclass {
                        "class"
                    } else {
                        "instance"
                    },
                    name,
                    cat_ptr,
                    if host_obj.is_metaclass {
                        "metaclass"
                    } else {
                        "class"
                    },
                    host_obj.name,
                    class,
                );
                host_obj.add_methods_from_bin(methods, mem, self);
                *self.borrow_mut::<ClassHostObject>(class) = host_obj;
            }
        }
    }

    pub fn class_is_subclass_of(&self, class: Class, superclass: Class) -> bool {
        if class == superclass {
            return true;
        }

        let mut class = class;
        loop {
            let &ClassHostObject {
                superclass: next, ..
            } = self.borrow(class);
            if next == nil {
                return false;
            } else if next == superclass {
                return true;
            } else {
                class = next;
            }
        }
    }

    pub fn get_class_name(&self, class: Class) -> &str {
        self.try_get_class_name(class)
            .expect("Could not get class name!")
    }

    /// Guest implementations of `selector` that `class` and its superclasses
    /// define themselves (not inherited), from the root class down. Used for
    /// `.cxx_construct` and `.cxx_destruct`, which each class only runs for
    /// its own ivars.
    pub fn own_guest_implementations(
        &self,
        class: Class,
        selector: SEL,
    ) -> Vec<crate::abi::GuestFunction> {
        let mut imps = Vec::new();
        let mut class = class;
        while class != nil {
            let Some(host_object) = self.get_host_object(class) else {
                break;
            };
            let Some(class_host_object) = host_object.as_any().downcast_ref::<ClassHostObject>()
            else {
                break;
            };
            if let Some(IMP::Guest(imp)) = class_host_object.methods.get(&selector) {
                imps.push(*imp);
            }
            class = class_host_object.superclass;
        }
        imps.reverse();
        imps
    }

    pub fn get_superclass(&self, class: Class) -> Class {
        let &ClassHostObject { superclass, .. } = self.borrow(class);
        superclass
    }

    pub fn try_get_class_name(&self, class: Class) -> Option<&str> {
        let host_object = self.get_host_object(class)?;
        if let Some(ClassHostObject { name, .. }) = host_object.as_any().downcast_ref() {
            Some(name)
        } else if let Some(UnimplementedClass { name, .. }) = host_object.as_any().downcast_ref() {
            Some(name)
        } else if let Some(FakeClass { name, .. }) = host_object.as_any().downcast_ref() {
            Some(name)
        } else {
            None
        }
    }

    pub fn is_unimplemented_class(&self, class: Class) -> bool {
        if class == nil {
            return false;
        }
        let host_object = self.get_host_object(class).unwrap();
        matches!(
            host_object.as_any().downcast_ref(),
            Some(UnimplementedClass { .. })
        )
    }

    pub fn is_fake_class(&self, class: Class) -> bool {
        if class == nil {
            return false;
        }
        let host_object = self.get_host_object(class).unwrap();
        matches!(host_object.as_any().downcast_ref(), Some(FakeClass { .. }))
    }
}

pub(super) fn objc_getClass(env: &mut Environment, name: ConstPtr<u8>) -> id {
    let name_str = env.mem.cstr_at_utf8(name).unwrap();
    env.objc
        .get_class(name_str, false, &env.mem)
        .unwrap_or_else(|| panic!("objc_getClass() for unimplemented class {name_str}"))
}

pub(super) fn class_getSuperclass(env: &mut Environment, cls: Class) -> Class {
    if cls == nil {
        nil
    } else {
        env.objc.borrow::<ClassHostObject>(cls).superclass
    }
}

pub(super) fn class_getInstanceSize(env: &mut Environment, cls: Class) -> GuestUSize {
    if cls == nil {
        0
    } else {
        env.objc.borrow::<ClassHostObject>(cls).instance_size
    }
}

pub(super) fn class_getProperty(
    env: &mut Environment,
    cls: Class,
    name: ConstPtr<u8>,
) -> objc_property_t {
    if cls == nil {
        return Ptr::null();
    }
    let c_name = env.mem.cstr_at_utf8(name).unwrap();
    let class_name_string = env.objc.get_class_name(cls).to_owned();
    if class_name_string == "UIScreen" && c_name == "scale" {
        // Even if [UIScreen scale] is implemented, we're not yet having a
        // proper support for `objc_property_t`, so we prefer to return a NULL
        // here (e.g. property is not declared).
        // Some games (such as Mirror's Edge) check for those to conditionally
        // apply some parameters depending on the iOS version without actually
        // using the property.
        // We also prefer to not define this as a game-specific hack, because
        // some other EA games may rely on the same logic.
        // TODO: support `objc_property_t` properly
        log!("TODO: class_getProperty(UIScreen, scale) -> NULL");
        return Ptr::null();
    }
    todo!()
}

pub(super) fn class_replaceMethod(
    env: &mut Environment,
    cls: Class,
    name: SEL,
    imp: IMP,
    types: ConstPtr<u8>,
) -> IMP {
    // TODO: avoid unnecessary copy of types
    let types_copy = strdup(env, types).cast_const();
    let &mut ClassHostObject {
        ref mut methods,
        ref mut guest_method_signatures,
        ..
    } = env.objc.borrow_mut(cls);
    if let Entry::Vacant(e) = methods.entry(name) {
        // TODO: use `class_addMethod` once implemented
        log_dbg!(
            "class_replaceMethod: adding new implementation {:?} for method {}",
            imp,
            name.as_str(&env.mem)
        );
        e.insert(imp);
        guest_method_signatures.insert(name, types_copy);
        return IMP::guest_null();
    }
    env.mem.free(types_copy.cast().cast_mut());
    // TODO: use `method_setImplementation` once implemented
    // Note: encoding types are ignored
    let existing = methods.insert(name, imp).unwrap();
    assert!(matches!(existing, IMP::Guest(_))); // TODO
    log_dbg!(
        "class_replaceMethod: existing {:?} replaced with {:?} for method {}",
        existing,
        imp,
        name.as_str(&env.mem)
    );
    existing
}

pub(super) fn class_getMethodImplementation(env: &mut Environment, cls: Class, name: SEL) -> IMP {
    if cls == nil {
        return IMP::guest_null();
    }
    let mut class = cls;
    loop {
        let &ClassHostObject {
            superclass: next,
            ref methods,
            ..
        } = env.objc.borrow(class);
        if methods.contains_key(&name) {
            let method = *methods.get(&name).unwrap();
            assert!(matches!(method, IMP::Guest(_))); // TODO
            return method;
        } else if next == nil {
            // TODO: currently this returns NULL for unimplemented host methods
            return IMP::guest_null();
        }
        class = next;
    }
}
pub fn ___objc_personality_v0(
    _env: &mut crate::Environment,
    version: i32,
    actions: i32,
    exception_class: u64,
    exception_object: MutVoidPtr,
    context: MutVoidPtr,
) -> i32 {
    log!(
        "___objc_personality_v0 called! Exception handling is not fully implemented in touchHLE.\n\
        version: {}, actions: {}, class: {:x}, object: {:?}, context: {:?}",
        version,
        actions,
        exception_class,
        exception_object,
        context
    );
    // _URC_FATAL_PHASE1_ERROR
    3
}

/// `BOOL class_addMethod(Class cls, SEL name, IMP imp, const char *types)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418901-class_addmethod?language=objc):
///
/// > Adds a new method to a class with a given name and implementation.
/// >
/// > Returns `YES` if the method was added successfully, otherwise `NO`
/// > (for example, the class already contains a method implementation
/// > with that name).
/// >
/// > `class_addMethod` will add an override of a superclass's
/// > implementation, but will not replace an existing implementation in
/// > this class. To change an existing implementation, use
/// > `method_setImplementation`.
///
/// We model `IMP` as a guest function pointer (the `Method` type returned
/// by `class_getInstanceMethod` is touchHLE-specific and is not the same
/// representation as `IMP`). The `types` string is captured into
/// `guest_method_signatures` so that subsequent dispatch through
/// `methodSignatureForSelector:` can honour it.
pub fn class_addMethod(
    env: &mut crate::Environment,
    cls: Class,
    name: SEL,
    imp: ConstVoidPtr,
    types: ConstPtr<u8>,
) -> bool {
    if cls.is_null() || name.is_null() {
        return false;
    }
    // Refuse to install a NULL IMP — calling it would crash the guest as
    // soon as the selector is dispatched, and Apple's runtime treats it as
    // a programmer error (the docs require `imp` to be a function pointer
    // of type `IMP`).
    if imp.is_null() {
        log!(
            "Warning: class_addMethod({:?}, {:?}, NULL imp, ...) — refusing to install NULL IMP",
            cls,
            name.as_str(&env.mem)
        );
        return false;
    }

    // Determine whether the receiver class already has an implementation
    // for this selector. Apple's contract is that `class_addMethod` only
    // succeeds when the method is NOT already defined on the receiver
    // (overrides of superclasses ARE allowed, hence we don't walk up the
    // chain here).
    let already_defined = if let Some(host_obj) = env.objc.get_host_object(cls) {
        host_obj
            .as_any()
            .downcast_ref::<ClassHostObject>()
            .is_some_and(|c| c.methods.contains_key(&name))
    } else {
        false
    };
    if already_defined {
        log_dbg!(
            "class_addMethod({:?}, {:?}, ...) — method already defined on receiver, returning NO",
            cls,
            name.as_str(&env.mem)
        );
        return false;
    }

    // Treat the IMP as a guest function pointer. `IMP` on real Apple is
    // `id (*)(id, SEL, ...)` — i.e. a C function pointer. Method names
    // ending with `:` are encoded for the Thumb bit by the linker, so we
    // pass the raw bits straight through.
    let guest_imp = crate::abi::GuestFunction::from_addr_with_thumb_bit(imp.to_bits());

    // Install the new method. `borrow_mut::<ClassHostObject>` walks any
    // host-object inheritance chain so this works for both classes and
    // metaclasses (whose host objects are both `ClassHostObject`).
    {
        let class_obj = env.objc.borrow_mut::<ClassHostObject>(cls);
        class_obj
            .methods
            .insert(name, super::methods::IMP::Guest(guest_imp));
        if !types.is_null() {
            class_obj.guest_method_signatures.insert(name, types);
        }
    }
    log_dbg!(
        "class_addMethod({:?}, {:?}, imp={:?}) — installed",
        cls,
        name.as_str(&env.mem),
        imp
    );
    true
}

/// `BOOL class_conformsToProtocol(Class cls, Protocol *protocol)`.
/// Checks protocols adopted by the class, its categories, and its superclasses.
pub fn class_conformsToProtocol(env: &mut crate::Environment, cls: Class, protocol: id) -> bool {
    class_conforms_to_protocol(&env.objc, &env.mem, cls, protocol)
}

/// `Ivar *class_copyIvarList(Class cls, unsigned int *outCount)` —
/// `class_getInstanceVariable` and `object_getIvar` use the original opaque
/// metadata pointers, but this class representation does not retain ivars in
/// declaration order, so this list API currently returns NULL with `*outCount =
/// 0`.
pub fn class_copyIvarList(
    env: &mut crate::Environment,
    _cls: Class,
    out_count: crate::mem::MutPtr<u32>,
) -> crate::mem::MutPtr<ConstVoidPtr> {
    if !out_count.is_null() {
        env.mem.write(out_count, 0);
    }
    Ptr::null()
}

/// `Method *class_copyMethodList(Class cls, unsigned int *outCount)` —
/// returns a malloc'd array of `Method` pointers (one per instance method
/// declared on `cls`). Per Apple's Objective-C Runtime Reference
/// (<https://developer.apple.com/documentation/objectivec/1418490-class_copymethodlist>):
///
/// > You must free the list with `free()`.
/// > If `cls` declares no instance methods, returns `NULL` and `*outCount` is
/// 0.
///
/// touchHLE's `class_getInstanceMethod` returns the class pointer (it
/// doesn't model real `Method` structs), so we mirror that by writing the
/// class pointer once per known selector — callers walking the array can
/// still use the pointers with
/// `method_getImplementation`/`method_setImplementation` or
/// `class_replaceMethod`, which expect this same opaque representation.
pub fn class_copyMethodList(
    env: &mut crate::Environment,
    cls: Class,
    out_count: crate::mem::MutPtr<u32>,
) -> crate::mem::MutPtr<ConstVoidPtr> {
    if !out_count.is_null() {
        env.mem.write(out_count, 0);
    }
    if cls.is_null() {
        return Ptr::null();
    }
    // Collect the selectors defined directly on `cls` (Apple's contract:
    // this call does NOT walk superclasses).
    let sels: Vec<SEL> = if let Some(host_obj) = env.objc.get_host_object(cls) {
        if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
            class_obj.methods.keys().copied().collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    if sels.is_empty() {
        return Ptr::null();
    }
    let total = (sels.len() as u32) * guest_size_of::<ConstVoidPtr>();
    let buf: crate::mem::MutPtr<ConstVoidPtr> = env.mem.alloc(total).cast();
    // Emit a real opaque `Method` handle per selector so callers can use each
    // entry with `method_getName` / `method_getImplementation` /
    // `method_setImplementation` and observe the correct per-method state.
    for (i, sel) in sels.iter().enumerate() {
        let handle = method_handle_for(env, cls, *sel);
        env.mem.write(buf + i as u32, handle);
    }
    if !out_count.is_null() {
        env.mem.write(out_count, sels.len() as u32);
    }
    buf
}

/// `objc_property_t *class_copyPropertyList(Class cls, unsigned int *outCount)`
/// — returns a NULL-terminated, malloc'd array of `objc_property_t` opaque
/// pointers describing every declared `@property` in `cls` (not in its
/// superclasses). Per Apple's Objective-C Runtime Reference
/// (<https://developer.apple.com/documentation/objectivec/1418553-class_copypropertylist>):
///
/// > You should use `free()` to free the array.
/// > If the class declares no properties, the function returns `NULL` and
/// > `*outCount` is `0`.
///
/// We model "declared properties" using the `ClassHostObject::properties`
/// map populated by the runtime when a `@property` directive is processed
/// for a class. Properties inherited from superclasses are intentionally
/// excluded, matching real Apple behaviour.
pub fn class_copyPropertyList(
    env: &mut crate::Environment,
    cls: Class,
    out_count: crate::mem::MutPtr<u32>,
) -> crate::mem::MutPtr<ConstVoidPtr> {
    if !out_count.is_null() {
        env.mem.write(out_count, 0);
    }
    if cls.is_null() {
        return Ptr::null();
    }
    // Collect property pointers WITHOUT walking the hierarchy —
    // class_getProperty walks it (matching the documented `class_getProperty`
    // semantics), but class_copyPropertyList must NOT, per Apple's runtime
    // reference.
    let entries: Vec<ConstVoidPtr> = if let Some(host_obj) = env.objc.get_host_object(cls) {
        if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
            class_obj.properties.values().copied().collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    if entries.is_empty() {
        return Ptr::null();
    }
    // Allocate a NULL-terminated array of `objc_property_t` in guest memory.
    // The Apple manpage doesn't explicitly say it's NULL-terminated, but real
    // libobjc returns a malloc'd array of exactly `*outCount` entries.
    let total = (entries.len() as u32) * guest_size_of::<ConstVoidPtr>();
    let buf: crate::mem::MutPtr<ConstVoidPtr> = env.mem.alloc(total).cast();
    for (i, e) in entries.iter().enumerate() {
        env.mem.write(buf + i as u32, *e);
    }
    if !out_count.is_null() {
        env.mem.write(out_count, entries.len() as u32);
    }
    buf
}

/// `Protocol * __unsafe_unretained *class_copyProtocolList(Class cls, unsigned
/// int *outCount)`. Returns only protocols adopted by `cls` and its categories,
/// not inherited protocols.
pub fn class_copyProtocolList(
    env: &mut crate::Environment,
    cls: Class,
    out_count: crate::mem::MutPtr<u32>,
) -> crate::mem::MutPtr<ConstVoidPtr> {
    if !out_count.is_null() {
        env.mem.write(out_count, 0);
    }
    if cls.is_null() {
        return Ptr::null();
    }
    let protocols = env
        .objc
        .get_host_object(cls)
        .and_then(|host| host.as_any().downcast_ref::<ClassHostObject>())
        .map(|class| class.protocols.clone())
        .unwrap_or_default();
    let Ok(count) = u32::try_from(protocols.len()) else {
        return Ptr::null();
    };
    if count == 0 {
        return Ptr::null();
    }
    let Some(size) = count.checked_mul(guest_size_of::<ConstVoidPtr>()) else {
        return Ptr::null();
    };
    let buffer: crate::mem::MutPtr<ConstVoidPtr> = env.mem.alloc(size).cast();
    for (index, protocol) in protocols.iter().enumerate() {
        env.mem
            .write(buffer + index as u32, protocol.cast_const().cast());
    }
    if !out_count.is_null() {
        env.mem.write(out_count, count);
    }
    buffer
}

/// `Method class_getClassMethod(Class cls, SEL name)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418680-class_getclassmethod?language=objc):
///
/// > Returns a pointer to the data structure describing the class method
/// > identified by the given selector.
/// >
/// > Note: this function searches superclasses for implementations.
///
/// In touchHLE's simplified runtime the returned "Method" value is the
/// class pointer itself (matching what `class_getInstanceMethod` returns).
/// Class methods live on the metaclass, so we resolve the metaclass first
/// and then walk its chain looking for the selector.
pub fn class_getClassMethod(env: &mut crate::Environment, cls: Class, name: SEL) -> ConstVoidPtr {
    if cls.is_null() || name.is_null() {
        return ConstVoidPtr::null();
    }
    // The metaclass holds the class-method table. `ObjC::read_isa` of a
    // class returns its metaclass.
    let mut curr = crate::objc::ObjC::read_isa(cls, &env.mem);
    let mut defining = nil;
    while !curr.is_null() {
        if let Some(host_obj) = env.objc.get_host_object(curr) {
            if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
                if class_obj.methods.contains_key(&name) {
                    defining = curr;
                    break;
                }
            }
        }
        let next = env.objc.get_superclass(curr);
        if next == curr {
            break;
        }
        curr = next;
    }
    if defining.is_null() {
        return ConstVoidPtr::null();
    }
    // Like `class_getInstanceMethod`, hand out the opaque `Method` handle
    // for the (defining metaclass, selector) pair — NOT the class pointer.
    // Guests feed the result to `method_getImplementation` /
    // `method_setImplementation` (the classic swizzling idiom, e.g.
    // ObjectAL's `SynthesizeSingleton` macro replacing `+sharedInstance`);
    // a raw class pointer fails to decode as a `Method` handle, so the
    // swizzle silently no-ops and the guest's "method did not get
    // swizzled" assertions fire forever.
    method_handle_for(env, defining, name)
}

/// `Method class_getInstanceMethod(Class cls, SEL name)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418889-class_getinstancemethod?language=objc):
///
/// > Returns a specified instance method for a given class, searching
/// > superclasses for the implementation. Returns `NULL` if `cls` is `Nil`,
/// > `name` is `NULL`, or instances of `cls` do not respond to `name`.
pub fn class_getInstanceMethod(
    env: &mut crate::Environment,
    cls: Class,
    name: SEL,
) -> ConstVoidPtr {
    if cls.is_null() || name.is_null() {
        return ConstVoidPtr::null();
    }
    let defining = find_defining_class(env, cls, name);
    if defining.is_null() {
        return ConstVoidPtr::null();
    }
    method_handle_for(env, defining, name)
}

/// `IMP class_getMethodImplementation_stret(Class cls, SEL name)` — same
/// as above but for selectors whose return type uses `objc_msgSend_stret`
/// (large struct returns). The IMP slot is the same in touchHLE.
pub fn class_getMethodImplementation_stret(
    env: &mut crate::Environment,
    cls: Class,
    name: crate::objc::SEL,
) -> ConstVoidPtr {
    if cls.is_null() || name.is_null() {
        return ConstVoidPtr::null();
    }
    let defining = find_defining_class(env, cls, name);
    if defining.is_null() {
        return ConstVoidPtr::null();
    }
    match imp_at(env, defining, name) {
        Some(imp) => imp_to_callable_guest_ptr(env, imp, "_objc_msgSend_stret"),
        None => ConstVoidPtr::null(),
    }
}

/// `const char *class_getName(Class cls)` — returns the C-string class
/// name. We allocate the cstring in guest memory the first time a name
/// is requested per class and cache the pointer in the side-table so
/// repeated calls return the same address (matching real ObjC's
/// guarantee that the returned pointer is stable for the lifetime of
/// the class).
pub fn class_getName(env: &mut crate::Environment, cls: Class) -> ConstPtr<u8> {
    use crate::mem::Ptr;
    if cls.is_null() {
        // Apple: returns "nil"; touchHLE returns the empty string
        // pointer to keep the call safe.
        return Ptr::null();
    }
    let name = env.objc.get_class_name(cls).to_owned();
    let bytes = name.as_bytes();
    let len: u32 = bytes.len() as u32 + 1;
    let buf: crate::mem::MutPtr<u8> = env.mem.alloc(len).cast();
    for (i, &b) in bytes.iter().enumerate() {
        env.mem.write(buf + i as u32, b);
    }
    env.mem.write(buf + bytes.len() as u32, 0);
    buf.cast_const()
}

/// `BOOL class_isMetaClass(Class cls)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418629-class_ismetaclass):
/// returns `YES` if `cls` is a metaclass, `NO` otherwise (including when
/// `cls` is `Nil` or not a class at all).
///
/// Chrome's networking layer calls this during Objective-C runtime
/// introspection.
pub fn class_isMetaClass(env: &mut crate::Environment, cls: Class) -> bool {
    if cls.is_null() {
        return false;
    }
    if let Some(host_obj) = env.objc.get_host_object(cls) {
        if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
            return class_obj.is_metaclass;
        }
        if let Some(unimpl) = host_obj.as_any().downcast_ref::<UnimplementedClass>() {
            return unimpl.is_metaclass;
        }
        if let Some(fake) = host_obj.as_any().downcast_ref::<FakeClass>() {
            return fake.is_metaclass;
        }
    }
    // Guest-defined class: read the class struct from guest memory and check
    // the CLS_META flag in its class_rw_t data (flags bit 0x2, matching the
    // legacy Objective-C runtime this project targets).
    let class_t_size = std::mem::size_of::<class_t>() as GuestUSize;
    if env
        .mem
        .get_bytes_fallible(cls.cast_const().cast(), class_t_size)
        .is_none()
    {
        return false;
    }
    let class_struct: class_t = env.mem.read(cls.cast());
    if class_struct.data.is_null() {
        return false;
    }
    let rw_size = std::mem::size_of::<class_rw_t>() as GuestUSize;
    if env
        .mem
        .get_bytes_fallible(class_struct.data.cast(), rw_size)
        .is_none()
    {
        return false;
    }
    let rw: class_rw_t = env.mem.read(class_struct.data.cast());
    (rw._flags & 0x2) != 0
}

/// `BOOL class_respondsToSelector(Class cls, SEL sel)`
///
/// Apple: "Returns a Boolean value that indicates whether instances of a
/// class respond to a particular selector." Walks the class chain and
/// reports YES if any class in that chain implements `sel`.
/// <https://developer.apple.com/documentation/objectivec/1418555-class_respondstoselector>
///
/// Per the documentation `class_respondsToSelector(Nil, …)` returns NO and
/// `class_respondsToSelector(cls, NULL)` also returns NO. Mirroring the
/// real runtime here avoids the previous return-0 stub silently breaking
/// guests that use this entry point to gate optional behaviour
/// (e.g. `class_respondsToSelector([NSString class], @selector(...))` for
/// runtime-availability checks in older SDKs).
pub fn class_respondsToSelector(env: &mut crate::Environment, cls: Class, sel: SEL) -> bool {
    if cls.is_null() || sel.is_null() {
        return false;
    }

    let mut curr = cls;
    while !curr.is_null() {
        if let Some(host_obj) = env.objc.get_host_object(curr) {
            if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
                if class_obj.methods.contains_key(&sel) {
                    return true;
                }
            }
        }
        let next = env.objc.get_superclass(curr);
        if next == curr {
            break;
        }
        curr = next;
    }
    false
}

/// `Class class_setSuperclass(Class cls, Class newSuper)` — deprecated
/// since OS X 10.5 but still exported by libobjc and used by older
/// hooking libraries. Returns the previous superclass and rewrites the
/// class's inheritance chain in-place.
/// <https://developer.apple.com/documentation/objectivec/1418687-class_setsuperclass>
pub fn class_setSuperclass(env: &mut crate::Environment, cls: Class, new_super: Class) -> Class {
    if cls.is_null() {
        return nil;
    }
    let old = env.objc.get_superclass(cls);
    env.objc.borrow_mut::<ClassHostObject>(cls).superclass = new_super;
    old
}

/// `IMP imp_implementationWithBlock(id block)` — returns an IMP that invokes
/// `block` when called. Per the Objective-C runtime documentation the block
/// is copied by the runtime (callers routinely pass stack blocks, e.g.
/// Crittercism's NSURLConnection hooks), and its signature is
/// `^(id self, method_args...)`: there is no `_cmd` argument. Like Apple's
/// `objc-blocktramps-arm.s`, the returned IMP therefore just overwrites
/// `_cmd` with `self` and `self` with the block, then tail-calls the
/// block's invoke function, leaving all other arguments (and callee-saved
/// registers) untouched:
///
/// ```text
/// mov r1, r0            ; _cmd = self
/// ldr r0, [pc, #0]      ; self = block literal
/// ldr pc, [r0, #12]     ; tail-call block->invoke (interworking)
/// <block literal>       ; literal pool
/// ```
///
/// Blocks flagged `BLOCK_USE_STRET` get the same sequence shifted by one
/// register, as their first argument is the struct-return pointer.
pub fn imp_implementationWithBlock(
    env: &mut crate::Environment,
    block: id,
) -> crate::mem::ConstVoidPtr {
    const BLOCK_USE_STRET: u32 = 1 << 29;

    if block.to_bits() == 0 {
        // `imp_implementationWithBlock(NULL)` is documented to crash the
        // process; returning NULL is the closest non-fatal behaviour.
        return crate::mem::ConstVoidPtr::from_bits(0);
    }
    let block = crate::libc::dispatch::_Block_copy(env, block.cast().cast_const());
    let flags: u32 = env.mem.read(block.cast::<u32>() + 1);
    let tramp: crate::mem::MutPtr<u32> = env.mem.alloc(16).cast();
    if flags & BLOCK_USE_STRET != 0 {
        env.mem.write(tramp + 0, 0xe1a0_2001); // mov r2, r1
        env.mem.write(tramp + 1, 0xe59f_1000); // ldr r1, [pc, #0]
        env.mem.write(tramp + 2, 0xe591_f00c); // ldr pc, [r1, #12]
    } else {
        env.mem.write(tramp + 0, 0xe1a0_1000); // mov r1, r0
        env.mem.write(tramp + 1, 0xe59f_0000); // ldr r0, [pc, #0]
        env.mem.write(tramp + 2, 0xe590_f00c); // ldr pc, [r0, #12]
    }
    env.mem.write(tramp + 3, block.to_bits()); // literal: block literal
    tramp.cast().cast_const()
}

/// `char *method_copyArgumentType(Method m, unsigned int index)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418642-method_copyargumenttype?language=objc):
///
/// > Returns a string describing a single parameter type of a method, or
/// > `NULL` if method has no parameter at the specified index. You must free
/// > the string with `free()`.
pub fn method_copyArgumentType(
    env: &mut crate::Environment,
    m: ConstVoidPtr,
    index: u32,
) -> ConstVoidPtr {
    if method_handle_decode(env, m).is_none() {
        return ConstVoidPtr::null();
    }
    let arg = method_type_encoding_bytes(env, m).and_then(|types| {
        super::type_encoding::method_argument_type(&types, index as usize).map(<[u8]>::to_vec)
    });
    match arg {
        Some(arg) => copy_type_string_to_guest(env, &arg),
        None => ConstVoidPtr::null(),
    }
}

/// `char *method_copyReturnType(Method m)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418783-method_copyreturntype?language=objc):
///
/// > Returns a string describing a method's return type. You must free the
/// > string with `free()`.
pub fn method_copyReturnType(env: &mut crate::Environment, m: ConstVoidPtr) -> ConstVoidPtr {
    if method_handle_decode(env, m).is_none() {
        return ConstVoidPtr::null();
    }
    let ret = method_type_encoding_bytes(env, m)
        .and_then(|types| super::type_encoding::method_return_type(&types).map(<[u8]>::to_vec));
    match ret {
        Some(ret) => copy_type_string_to_guest(env, &ret),
        None => ConstVoidPtr::null(),
    }
}

/// `void method_exchangeImplementations(Method m1, Method m2)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418769-method_exchangeimplementations?language=objc):
///
/// > Exchanges the implementations of two methods. This is an atomic
/// > version of the following:
/// > ```text
/// > IMP imp1 = method_getImplementation(m1);
/// > IMP imp2 = method_getImplementation(m2);
/// > method_setImplementation(m1, imp2);
/// > method_setImplementation(m2, imp1);
/// > ```
///
/// Because our `Method` handles encode the defining class and selector, we
/// can swap the real entries in each class's method table (along with their
/// type-encoding strings). This makes method swizzling take effect — e.g.
/// cocos2d's `SynthesizeSingleton` macro, whose `NSAssert` used to fail on
/// every frame because the previous implementation was a no-op.
pub fn method_exchangeImplementations(
    env: &mut crate::Environment,
    m1: ConstVoidPtr,
    m2: ConstVoidPtr,
) {
    let (Some((cls1, sel1)), Some((cls2, sel2))) =
        (method_handle_decode(env, m1), method_handle_decode(env, m2))
    else {
        log!(
            "Warning: method_exchangeImplementations({:?}, {:?}) — unknown Method handle(s); ignoring.",
            m1,
            m2
        );
        return;
    };

    let imp1 = imp_at(env, cls1, sel1);
    let imp2 = imp_at(env, cls2, sel2);

    // Swap type-encoding strings too, so `method_getTypeEncoding` keeps
    // reporting the encoding that matches each installed implementation.
    let types1 = env
        .objc
        .get_host_object(cls1)
        .and_then(|h| h.as_any().downcast_ref::<ClassHostObject>())
        .and_then(|c| c.guest_method_signatures.get(&sel1).copied());
    let types2 = env
        .objc
        .get_host_object(cls2)
        .and_then(|h| h.as_any().downcast_ref::<ClassHostObject>())
        .and_then(|c| c.guest_method_signatures.get(&sel2).copied());

    if let Some(imp) = imp2 {
        set_imp_at(env, cls1, sel1, imp);
    }
    if let Some(imp) = imp1 {
        set_imp_at(env, cls2, sel2, imp);
    }
    {
        let class_obj = env.objc.borrow_mut::<ClassHostObject>(cls1);
        match types2 {
            Some(t) => {
                class_obj.guest_method_signatures.insert(sel1, t);
            }
            None => {
                class_obj.guest_method_signatures.remove(&sel1);
            }
        }
    }
    {
        let class_obj = env.objc.borrow_mut::<ClassHostObject>(cls2);
        match types1 {
            Some(t) => {
                class_obj.guest_method_signatures.insert(sel2, t);
            }
            None => {
                class_obj.guest_method_signatures.remove(&sel2);
            }
        }
    }

    log_dbg!(
        "method_exchangeImplementations({:?}, {:?}) — swapped [{:?} {:?}] <-> [{:?} {:?}]",
        m1,
        m2,
        cls1,
        sel1,
        cls2,
        sel2
    );
}

/// `void method_getArgumentType(Method m, unsigned int index, char *dst,
///                              size_t dst_len)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418989-method_getargumenttype?language=objc):
///
/// > The parameter type string is copied to `dst`. `dst` is filled as if
/// > `strncpy(dst, parameter_type, dst_len)` were called. If the method
/// > contains no parameter with that index, `dst` is filled as if
/// > `strncpy(dst, "", dst_len)` were called.
pub fn method_getArgumentType(
    env: &mut crate::Environment,
    m: ConstVoidPtr,
    index: u32,
    dst: MutVoidPtr,
    dst_len: GuestUSize,
) {
    let arg = if method_handle_decode(env, m).is_some() {
        method_type_encoding_bytes(env, m).and_then(|types| {
            super::type_encoding::method_argument_type(&types, index as usize).map(<[u8]>::to_vec)
        })
    } else {
        None
    };
    strncpy_type_string_to_guest(env, arg.as_deref().unwrap_or(&[]), dst, dst_len);
}

/// `IMP method_getImplementation(Method m)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418551-method_getimplementation?language=objc):
///
/// > Returns the implementation of a method.
///
/// `m` is one of our opaque `Method` handles (see [method_handle_for]); we
/// decode it to a (class, selector) pair and return the guest function
/// pointer of the installed implementation.
pub fn method_getImplementation(env: &mut crate::Environment, m: ConstVoidPtr) -> ConstVoidPtr {
    let Some((cls, sel)) = method_handle_decode(env, m) else {
        return ConstVoidPtr::null();
    };
    match imp_at(env, cls, sel) {
        Some(imp) => imp_to_guest_ptr(env, cls, sel, imp),
        None => ConstVoidPtr::null(),
    }
}

/// `SEL method_getName(Method m)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418758-method_getname?language=objc):
///
/// > Returns the name of a method (the selector it is registered under).
pub fn method_getName(env: &mut crate::Environment, m: ConstVoidPtr) -> SEL {
    match method_handle_decode(env, m) {
        Some((_cls, sel)) => sel,
        None => SEL::null(),
    }
}

/// `unsigned int method_getNumberOfArguments(Method m)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418968-method_getnumberofarguments?language=objc):
///
/// > Returns the number of arguments accepted by a method.
///
/// The count includes the implicit `self` and `_cmd` arguments, and is
/// derived from the method's type encoding exactly like objc4's
/// `encoding_getNumberOfArguments`. Methods implemented by the host have no
/// recorded encoding; for those the count follows from the selector (one
/// argument per `:`, plus `self` and `_cmd`), which is the same rule
/// `-[NSObject methodSignatureForSelector:]` uses here.
pub fn method_getNumberOfArguments(env: &mut crate::Environment, m: ConstVoidPtr) -> u32 {
    let Some((_cls, sel)) = method_handle_decode(env, m) else {
        return 0;
    };
    if let Some(types) = method_type_encoding_bytes(env, m) {
        return super::type_encoding::method_argument_count(&types) as u32;
    }
    let colons = sel.as_str(&env.mem).bytes().filter(|&b| b == b':').count();
    (colons + 2) as u32
}

/// `void method_getReturnType(Method m, char *dst, size_t dst_len)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418617-method_getreturntype?language=objc):
///
/// > The method's return type string is copied to `dst`. `dst` is filled as
/// > if `strncpy(dst, parameter_type, dst_len)` were called.
pub fn method_getReturnType(
    env: &mut crate::Environment,
    m: ConstVoidPtr,
    dst: MutVoidPtr,
    dst_len: GuestUSize,
) {
    let ret = if method_handle_decode(env, m).is_some() {
        method_type_encoding_bytes(env, m)
            .and_then(|types| super::type_encoding::method_return_type(&types).map(<[u8]>::to_vec))
    } else {
        None
    };
    strncpy_type_string_to_guest(env, ret.as_deref().unwrap_or(&[]), dst, dst_len);
}

/// `const char *method_getTypeEncoding(Method m)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418488-method_gettypeencoding?language=objc):
///
/// > Returns a string describing a method's parameter and return types.
///
/// We return the guest pointer to the method's stored type-encoding string
/// (as parsed from the binary or supplied via `class_addMethod`), or NULL if
/// none is recorded.
pub fn method_getTypeEncoding(env: &mut crate::Environment, m: ConstVoidPtr) -> ConstVoidPtr {
    let Some((cls, sel)) = method_handle_decode(env, m) else {
        return ConstVoidPtr::null();
    };
    let types = env
        .objc
        .get_host_object(cls)
        .and_then(|h| h.as_any().downcast_ref::<ClassHostObject>())
        .and_then(|c| c.guest_method_signatures.get(&sel).copied());
    match types {
        Some(ptr) => ptr.cast(),
        None => ConstVoidPtr::null(),
    }
}

/// `IMP method_setImplementation(Method m, IMP imp)`
///
/// Per Apple's [Objective-C Runtime Reference](https://developer.apple.com/documentation/objectivec/1418707-method_setimplementation?language=objc):
///
/// > Sets the implementation of a method. Returns the previous
/// > implementation of the method.
///
/// This is one half of the classic swizzling idiom (the other being
/// `method_exchangeImplementations`). Decoding the `Method` handle lets us
/// mutate the real method table so the change is observed by subsequent
/// dispatch — exactly what patterns like cocos2d's `SynthesizeSingleton`
/// depend on.
pub fn method_setImplementation(
    env: &mut crate::Environment,
    m: ConstVoidPtr,
    imp: ConstVoidPtr,
) -> ConstVoidPtr {
    let Some((cls, sel)) = method_handle_decode(env, m) else {
        return ConstVoidPtr::null();
    };
    let old = imp_at(env, cls, sel);
    if let Some(new_imp) = guest_ptr_to_imp(env, imp) {
        set_imp_at(env, cls, sel, new_imp);
    }
    match old {
        Some(old_imp) => imp_to_guest_ptr(env, cls, sel, old_imp),
        None => ConstVoidPtr::null(),
    }
}

/// `id objc_alloc(Class cls)` — ARC/libobjc fast-path allocator. Apple's
/// objc4 runtime (`NSObject.mm`, `objc_alloc`) implements this as
/// `callAlloc(cls, /*checkNil=*/true, /*allocWithZone=*/false)`, which for a
/// class that does not override `+allocWithZone:` is exactly equivalent to
/// `[cls alloc]`. Forwarding the `alloc` selector reproduces that behaviour
/// precisely, including honouring any class that provides its own `+alloc`.
/// Compilers emit calls to this helper instead of an `objc_msgSend(cls,
/// @selector(alloc))` to save a selector lookup, so it must really allocate
/// — a return-0 stub leaves the guest with a nil object and crashes later.
pub fn objc_alloc(env: &mut crate::Environment, cls: Class) -> id {
    if cls.is_null() {
        return nil;
    }
    crate::objc::msg![env; cls alloc]
}

pub fn objc_allocWithZone(
    env: &mut crate::Environment,
    cls: Class,
    zone: crate::mem::MutVoidPtr,
) -> id {
    if cls.is_null() {
        return nil;
    }
    crate::objc::msg![env; cls allocWithZone:zone]
}

/// `Class objc_allocateClassPair(Class superclass, const char *name,
/// size_t extraBytes)` — used to dynamically create a new class.
/// Implementing this properly requires wiring into our `ClassHostObject`
/// machinery; until that's done, we install a placeholder
/// (UnimplementedClass) so the symbol resolves and the guest can decide
/// to fall back. Calls to `+[NewClass alloc]` etc. will go through the
/// usual UnimplementedClass path.
pub fn objc_allocateClassPair(
    env: &mut crate::Environment,
    _superclass: Class,
    name: ConstPtr<u8>,
    _extra_bytes: u32,
) -> Class {
    let Ok(s) = env.mem.cstr_at_utf8(name) else {
        return nil;
    };
    let name_str = s.to_string();
    log!(
        "Warning: objc_allocateClassPair({:?}, …): dynamic class registration is \
         not fully supported; installing an UnimplementedClass placeholder.",
        name_str
    );
    env.objc
        .link_class(&name_str, /* is_metaclass: */ false, &mut env.mem)
}

/// `id objc_autorelease(id obj)` — ARC/libobjc fast-path autorelease. objc4
/// implements it as `obj ? obj->autorelease() : nil`: the object is added to
/// the current autorelease pool and returned unchanged. touchHLE's
/// `autorelease` helper does exactly this (with a nil fast path), so we
/// forward to it. This is the real implementation, not a stub: returning 0
/// here would hand the guest a nil where it expects its object back.
pub fn objc_autorelease(env: &mut crate::Environment, obj: id) -> id {
    crate::objc::autorelease(env, obj)
}

pub fn objc_autoreleaseReturnValue(env: &mut crate::Environment, obj: id) -> id {
    crate::objc::autorelease(env, obj)
}

pub fn objc_begin_catch(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    if name.is_null() {
        return nil;
    }

    let name_str = match env.mem.cstr_at_utf8(name) {
        Ok(s) => s.to_string(),
        Err(_) => return nil,
    };
    if let Some(class) = env.objc.get_class(&name_str, false, &env.mem) {
        return class;
    }

    if ObjC::find_template(&name_str).is_some() {
        return env.objc.link_class(&name_str, false, &mut env.mem);
    }

    nil
}

/// `Class *objc_copyClassList(unsigned int *outCount)`.
/// Returns a malloc-allocated array containing every currently registered
/// class.
pub fn objc_copyClassList(
    env: &mut crate::Environment,
    out_count: crate::mem::MutPtr<u32>,
) -> crate::mem::MutPtr<Class> {
    let classes: Vec<Class> = env.objc.classes.values().copied().collect();
    copy_class_list_to_guest(&mut env.mem, &classes, out_count)
}

/// `const char **objc_copyClassNamesForImage(const char *image,
/// unsigned int *outCount)` — enumerate class names defined by a
/// loaded image. We don't track image membership, so write 0 to
/// `outCount` and return NULL. Apps either skip enumeration or hit a
/// safe early-exit.
pub fn objc_copyClassNamesForImage(
    env: &mut crate::Environment,
    _image: ConstPtr<u8>,
    out_count: crate::mem::MutPtr<u32>,
) -> ConstPtr<u8> {
    use crate::mem::Ptr;
    if !out_count.is_null() {
        env.mem.write(out_count, 0);
    }
    Ptr::null()
}

/// `const char **objc_copyImageNames(unsigned int *outCount)` — the names of
/// all loaded images, as a malloc-allocated array of C strings, with the
/// count written to `outCount`. Firebase's FIRCoreDiagnostics calls this at
/// launch and loops over `outCount` entries, so `outCount` must always be
/// written: leaving it unset (as the old return-0 stub did) made Stickman
/// Rope Hero walk billions of garbage pointers and never draw a frame.
pub fn objc_copyImageNames(
    env: &mut crate::Environment,
    out_count: crate::mem::MutPtr<u32>,
) -> crate::mem::MutPtr<ConstPtr<u8>> {
    use crate::mem::Ptr;
    let names: Vec<String> = env.bins.iter().map(|bin| bin.name.clone()).collect();
    let count = names.len() as u32;
    if !out_count.is_null() {
        env.mem.write(out_count, count);
    }
    if count == 0 {
        return Ptr::null();
    }
    let array: crate::mem::MutPtr<ConstPtr<u8>> = env
        .mem
        .alloc(count * guest_size_of::<ConstPtr<u8>>())
        .cast();
    for (index, name) in names.iter().enumerate() {
        let name_ptr = env.mem.alloc_and_write_cstr(name.as_bytes());
        env.mem.write(array + index as u32, name_ptr.cast_const());
    }
    array
}

/// `void objc_disposeClassPair(Class cls)` — destroys a class created
/// with [`objc_allocateClassPair`] that has not yet been registered.
/// touchHLE doesn't reclaim class storage, so we just drop the reference
/// on the floor (Apple's runtime is also lazy about this for very small
/// allocations).
/// <https://developer.apple.com/documentation/objectivec/1418912-objc_disposeclasspair>
pub fn objc_disposeClassPair(_env: &mut crate::Environment, _cls: Class) {}

pub fn objc_end_catch(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    if name.is_null() {
        return nil;
    }

    let name_str = match env.mem.cstr_at_utf8(name) {
        Ok(s) => s.to_string(),
        Err(_) => return nil,
    };
    if let Some(class) = env.objc.get_class(&name_str, false, &env.mem) {
        return class;
    }

    if ObjC::find_template(&name_str).is_some() {
        return env.objc.link_class(&name_str, false, &mut env.mem);
    }

    nil
}

/// `int objc_getClassList(Class *buffer, int bufferLen)` per Apple's
/// Objective-C Runtime Reference
/// (<https://developer.apple.com/documentation/objectivec/1418579-objc_getclasslist>):
///
/// > Returns the number of currently registered classes. If `buffer` is
/// > NULL or `bufferLen` is 0, it must just return the total count.
/// > Otherwise it must copy up to `bufferLen` Class pointers into
/// > `buffer`, but still return the total number registered (callers
/// > use that to detect that they need a bigger buffer).
#[allow(clippy::explicit_counter_loop)]
pub fn objc_getClassList(
    env: &mut crate::Environment,
    buffer: crate::mem::MutPtr<Class>,
    buffer_len: i32,
) -> i32 {
    let total = env.objc.classes.values().count();
    if buffer.is_null() || buffer_len <= 0 {
        return i32::try_from(total).unwrap_or(i32::MAX);
    }
    let cap = u32::try_from(buffer_len).unwrap_or(0);
    let mut offset: u32 = 0;
    for class in env.objc.classes.values() {
        if offset >= cap {
            break;
        }
        env.mem.write(buffer + offset, *class);
        offset += 1;
    }
    i32::try_from(total).unwrap_or(i32::MAX)
}

pub fn objc_getMetaClass(env: &mut crate::Environment, cls: Class, name: SEL) -> ConstVoidPtr {
    if cls.is_null() {
        return ConstVoidPtr::null();
    }

    let mut curr = cls;
    while !curr.is_null() {
        if let Some(host_obj) = env.objc.get_host_object(curr) {
            if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
                if class_obj.methods.contains_key(&name) {
                    return curr.cast_const().cast();
                }
            }
        }
        let next = env.objc.get_superclass(curr);
        if next == curr {
            break;
        }
        curr = next;
    }
    ConstVoidPtr::null()
}

/// `Protocol *objc_getProtocol(const char *name)` — the runtime does not
/// maintain a global registry for protocol definitions that are not adopted
/// by a loaded class, so name-based lookup is unavailable.
pub fn objc_getProtocol(_env: &mut crate::Environment, _name: ConstPtr<u8>) -> id {
    nil
}

/// `Class objc_getRequiredClass(const char *name)` — same as
/// `objc_getClass` but documented to abort on miss. Our runtime is
/// best-effort, so we just degrade to the lenient lookup; if the
/// class is genuinely missing, returning nil keeps the guest alive
/// instead of taking the host down.
pub fn objc_getRequiredClass(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    let class = objc_getClass(env, name);
    if class == nil {
        if let Ok(s) = env.mem.cstr_at_utf8(name) {
            log!(
                "Warning: objc_getRequiredClass({:?}): class not found; \
                 returning nil (the real runtime would abort).",
                s
            );
        }
    }
    class
}

/// `Class objc_lookUpClass(const char *name)` — like `objc_getClass`
/// but does *not* invoke the class handler if the class is missing
/// (returns nil instead of trying to load it). For our HLE runtime
/// the behaviour is identical to `objc_getClass`.
pub fn objc_lookUpClass(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    objc_getClass(env, name)
}

/// `Class objc_readClassPair(Class cls, const struct objc_image_info *info)`
/// — iOS 8+ Swift-related helper. Apps from iOS 6/7 may import the symbol but
/// rarely actually call it. Return the input class unchanged so any best-effort
/// caller keeps working.
pub fn objc_readClassPair(_env: &mut crate::Environment, cls: Class, _info: ConstVoidPtr) -> Class {
    cls
}

/// `void objc_registerClassPair(Class cls)` — finalise the registration
/// of a dynamically created class. touchHLE's [`objc_allocateClassPair`]
/// already inserts the class into the runtime tables; the matching
/// `register` call is therefore a no-op that just publishes the class.
/// <https://developer.apple.com/documentation/objectivec/1418414-objc_registerclasspair>
pub fn objc_registerClassPair(_env: &mut crate::Environment, _cls: Class) {}

pub fn objc_retain(env: &mut crate::Environment, obj: id) -> id {
    if !obj.is_null() {
        crate::objc::retain(env, obj);
    }
    obj
}

pub fn objc_retainAutorelease(env: &mut crate::Environment, obj: id) -> id {
    if !obj.is_null() {
        crate::objc::retain(env, obj);
        crate::objc::autorelease(env, obj);
    }
    obj
}

pub fn objc_retainAutoreleaseReturnValue(env: &mut crate::Environment, obj: id) -> id {
    objc_retainAutorelease(env, obj)
}

/// ARC return-value helpers use the ordinary retain/autorelease fallback.
/// We do not implement the caller/callee optimisation that elides the pair.
/// Their argument is an object, not a C string naming an Objective-C class.
pub fn objc_retainAutoreleasedReturnValue(env: &mut crate::Environment, obj: id) -> id {
    crate::objc::retain(env, obj)
}

/// `id objc_retainBlock(id block)` — ARC runtime helper for retaining a
/// block. Apple's objc4 runtime (`NSObject.mm`) implements this as a thin
/// wrapper around the Blocks runtime:
///
/// ```text
/// id objc_retainBlock(id x) { return (id)_Block_copy(x); }
/// ```
///
/// ARC emits this whenever a block is stored, so a stack block has to be
/// promoted to the heap here like in `_Block_copy` (see `src/libc/blocks.rs`).
/// Returning the stack block itself left a dangling pointer once its frame
/// was gone: Bolts' `BFExecutor` (Charlie, the Steak) then jumped to whatever
/// the reused stack held in place of the invoke function.
pub fn objc_retainBlock(env: &mut crate::Environment, block: id) -> id {
    let copy = crate::libc::dispatch::_Block_copy(env, block.cast().cast_const());
    copy.cast()
}

/// `void objc_storeStrong(id *location, id obj)` — ARC's strong-store
/// runtime helper. The clang ARC specification
/// (<https://clang.llvm.org/docs/AutomaticReferenceCounting.html#runtime-support>)
/// describes the canonical implementation as:
///
/// ```text
/// id prev = *location;
/// if (prev == obj) return;
/// objc_retain(obj);
/// *location = obj;
/// objc_release(prev);
/// ```
///
/// This is what the Apple objc runtime ships as `objc_storeStrong`. We
/// implement it line-by-line so the reference counts of both the old
/// and new objects stay consistent with how a non-ARC manual
/// retain/release would have managed them.
pub fn objc_storeStrong(env: &mut crate::Environment, location: crate::mem::MutPtr<id>, obj: id) {
    static TRACE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if location.is_null() {
        return;
    }
    let prev = env.mem.read(location);
    if crate::env_flag_cached!("TOUCHHLE_TRACE_IVARS")
        && TRACE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 256
    {
        log!(
            "ARC-STORE location={:?} prev={:?} new={:?}",
            location,
            prev,
            obj
        );
    }
    if prev == obj {
        return;
    }
    if !obj.is_null() {
        crate::objc::retain(env, obj);
    }
    env.mem.write(location, obj);
    if !prev.is_null() {
        crate::objc::release(env, prev);
    }
}

/// `id objc_unsafeClaimAutoreleasedReturnValue(id obj)` — ARC runtime
/// optimisation counterpart to `objc_retainAutoreleasedReturnValue`. Apple's
/// objc4 runtime uses it when a returned object is consumed by code that does
/// *not* want an owning reference (e.g. the result is immediately passed on,
/// or stored `__unsafe_unretained`): it accepts the autoreleased value and
/// claims it without adding a retain, balancing the optimised return sequence.
/// In touchHLE's serialised execution model there is no retain/autorelease
/// elision to undo, so the correct behaviour is to return the object
/// unchanged (and nil-safe). See `objc4` `NSObject.mm`,
/// `objc_unsafeClaimAutoreleasedReturnValue`.
pub fn objc_unsafeClaimAutoreleasedReturnValue(env: &mut crate::Environment, obj: id) -> id {
    let _ = env;
    obj
}

pub fn object_getClassName(env: &mut crate::Environment, obj: id) -> Class {
    if obj.is_null() {
        return nil;
    }

    let objc_obj: objc_object = env.mem.read(obj.cast());
    objc_obj.isa
}

/// `void *object_getIndexedIvars(id obj)` — returns a pointer to the
/// extra bytes allocated for a class created with
/// `objc_allocateClassPair(super, name, extraBytes)`. We don't allocate
/// extra bytes (see [objc_allocateClassPair]), so just return the
/// object pointer itself; callers that try to use it as scratch space
/// will get a no-op buffer rather than crashing on NULL.
pub fn object_getIndexedIvars(_env: &mut crate::Environment, obj: id) -> ConstVoidPtr {
    obj.cast().cast_const()
}

/// `BOOL protocol_conformsToProtocol(Protocol *proto, Protocol *other)`
/// per Apple's Objective-C Runtime Reference
/// (<https://developer.apple.com/documentation/objectivec/1418841-protocol_conformstoprotocol>):
///
/// > Returns a Boolean value that indicates whether one protocol
/// > conforms to another.
pub fn protocol_conformsToProtocol(env: &mut crate::Environment, proto: id, other: id) -> bool {
    protocol_conforms_to_protocol(&env.mem, proto, other, 0, &mut Vec::new())
}

/// `struct objc_method_description protocol_getMethodDescription(Protocol *p,
/// SEL aSel, BOOL isRequiredMethod, BOOL isInstanceMethod)`.
///
/// Protocol pointers the guest has are the `protocol_t` structures in its own
/// binary (touchHLE doesn't fix them up), so this walks their method lists,
/// and those of the protocols they adopt. Returns `{NULL, NULL}` if the
/// method isn't found. Crittercism uses this to find out which
/// `NSURLConnection` delegate methods to hook; the struct is returned via a
/// hidden pointer, so this mustn't be left to a return-0 stub.
pub fn protocol_getMethodDescription(
    env: &mut crate::Environment,
    proto: id,
    sel: SEL,
    is_required: bool,
    is_instance: bool,
) -> objc_method_description {
    let types = find_protocol_method_types(&env.mem, proto, sel, is_required, is_instance, 0);
    if types.is_null() {
        return objc_method_description {
            name: SEL::null(),
            types: Ptr::null(),
        };
    }
    objc_method_description { name: sel, types }
}

/// `const char *protocol_getName(Protocol *p)`.
pub fn protocol_getName(env: &mut crate::Environment, protocol: id) -> ConstPtr<u8> {
    protocol_get_name(&env.mem, protocol)
}

pub fn swift_getInitializedObjCClass(env: &mut crate::Environment, cls: Class) -> Class {
    if cls.is_null() {
        return nil;
    }
    let initialize_sel = env.objc.lookup_selector("initialize").unwrap_or_else(|| {
        env.objc
            .register_host_selector("initialize".to_string(), &mut env.mem)
    });
    if env.objc.object_has_method(&env.mem, cls, initialize_sel) {
        let _: id = crate::objc::msg_send_no_initialize(env, (cls, initialize_sel));
    }
    cls
}

fn class_conforms_to_protocol(objc: &ObjC, mem: &Mem, cls: Class, protocol: id) -> bool {
    if cls.is_null() || protocol.is_null() {
        return false;
    }
    let mut current = cls;
    let mut visited = Vec::new();
    while !current.is_null() && !visited.contains(&current) {
        visited.push(current);
        let Some(host_object) = objc.get_host_object(current) else {
            return false;
        };
        let Some(class_object) = host_object.as_any().downcast_ref::<ClassHostObject>() else {
            return false;
        };
        let superclass = class_object.superclass;
        let protocols = class_object.protocols.clone();
        if protocols.into_iter().any(|adopted| {
            protocol_conforms_to_protocol(mem, adopted, protocol, 0, &mut Vec::new())
        }) {
            return true;
        }
        current = superclass;
    }
    false
}

fn copy_class_list_to_guest(
    mem: &mut Mem,
    classes: &[Class],
    out_count: crate::mem::MutPtr<u32>,
) -> crate::mem::MutPtr<Class> {
    if !out_count.is_null() {
        mem.write(out_count, 0);
    }
    let Ok(count) = u32::try_from(classes.len()) else {
        return Ptr::null();
    };
    if count == 0 {
        return Ptr::null();
    }
    let Some(size) = count.checked_mul(guest_size_of::<Class>()) else {
        return Ptr::null();
    };
    let buffer: crate::mem::MutPtr<Class> = mem.alloc(size).cast();
    for (index, class) in classes.iter().enumerate() {
        mem.write(buffer + index as u32, *class);
    }
    if !out_count.is_null() {
        mem.write(out_count, count);
    }
    buffer
}

/// Copy `bytes` into a fresh `malloc`-compatible, NUL-terminated guest
/// string, as the `method_copy*Type` functions must return.
fn copy_type_string_to_guest(env: &mut crate::Environment, bytes: &[u8]) -> ConstVoidPtr {
    env.mem.alloc_and_write_cstr(bytes).cast_const().cast()
}

/// Returns the nearest class in `cls`'s superclass chain that defines an
/// uninherited implementation of `sel`, or `nil` if none does.
fn find_defining_class(env: &crate::Environment, cls: Class, sel: SEL) -> Class {
    let mut curr = cls;
    while !curr.is_null() {
        if let Some(host_obj) = env.objc.get_host_object(curr) {
            if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
                if class_obj.methods.contains_key(&sel) {
                    return curr;
                }
            }
        }
        let next = env.objc.get_superclass(curr);
        if next == curr {
            break;
        }
        curr = next;
    }
    nil
}

fn find_protocol_method_types(
    mem: &Mem,
    proto: id,
    sel: SEL,
    is_required: bool,
    is_instance: bool,
    depth: u32,
) -> ConstPtr<u8> {
    // Layout of the 32-bit `protocol_t`, in words: isa, name, protocols,
    // instanceMethods, classMethods, optionalInstanceMethods,
    // optionalClassMethods, ...
    if proto.is_null() || sel.is_null() || depth > 16 {
        return Ptr::null();
    }
    let words: ConstPtr<u32> = Ptr::from_bits(proto.to_bits());
    let list_index = match (is_required, is_instance) {
        (true, true) => 3,
        (true, false) => 4,
        (false, true) => 5,
        (false, false) => 6,
    };
    let list: ConstPtr<u32> = Ptr::from_bits(mem.read(words + list_index));
    if !list.is_null() {
        // `method_list_t`: entsize (low bits are flags), count, then entries
        // of name, types, imp.
        let entsize = (mem.read(list) & !3).max(8);
        let count: u32 = mem.read(list + 1);
        let sel_name = sel.as_str(mem).as_bytes();
        for i in 0..count.min(4096) {
            let entry: ConstPtr<ConstPtr<u8>> = Ptr::from_bits(list.to_bits() + 8 + i * entsize);
            let name = mem.read(entry);
            if !name.is_null() && mem.cstr_eq(name, sel_name) {
                return mem.read(entry + 1);
            }
        }
    }
    // `protocol_list_t`: count, then that many protocol pointers.
    let adopted: ConstPtr<u32> = Ptr::from_bits(mem.read(words + 2));
    if !adopted.is_null() {
        let count: u32 = mem.read(adopted);
        for i in 0..count.min(256) {
            let other: id = Ptr::from_bits(mem.read(adopted + 1 + i));
            let types =
                find_protocol_method_types(mem, other, sel, is_required, is_instance, depth + 1);
            if !types.is_null() {
                return types;
            }
        }
    }
    Ptr::null()
}

/// Convert a guest `IMP` pointer received from the guest back into an [IMP].
/// A pointer previously handed out for a host implementation is looked up in
/// the token registry; anything else is treated as a guest code address.
fn guest_ptr_to_imp(env: &crate::Environment, imp: ConstVoidPtr) -> Option<IMP> {
    if imp.is_null() {
        return None;
    }
    if let Some(host_imp) = env.objc.imp_tokens.get(&imp.to_bits()) {
        return Some(*host_imp);
    }
    Some(IMP::Guest(
        crate::abi::GuestFunction::from_addr_with_thumb_bit(imp.to_bits()),
    ))
}

/// Read the [IMP] stored for `sel` directly on `cls` (no inheritance walk).
fn imp_at(env: &crate::Environment, cls: Class, sel: SEL) -> Option<IMP> {
    env.objc
        .get_host_object(cls)
        .and_then(|h| h.as_any().downcast_ref::<ClassHostObject>())
        .and_then(|c| c.methods.get(&sel).copied())
}

fn imp_to_callable_guest_ptr(
    env: &mut crate::Environment,
    imp: IMP,
    message_send_symbol: &str,
) -> ConstVoidPtr {
    match imp {
        IMP::Guest(guest_function) => guest_function.to_ptr(),
        IMP::Host(_) => {
            let dyld = &mut env.dyld;
            let mem = &mut env.mem;
            let cpu = &mut env.cpu;
            dyld.create_proc_address(mem, cpu, message_send_symbol)
                .map(|guest_function| ConstVoidPtr::from_bits(guest_function.addr_with_thumb_bit()))
                .unwrap_or_else(|_| ConstVoidPtr::null())
        }
    }
}

/// Convert an [IMP] into the guest pointer that Apple's `IMP` type is (a
/// function pointer). Guest implementations expose their real code address;
/// host (Rust-backed) implementations have no guest address, so we mint a
/// stable, callable stub for the host IMP and remember which IMP it stands
/// for, allowing it to be round-tripped back through
/// `method_setImplementation` / `method_exchangeImplementations`.
fn imp_to_guest_ptr(
    env: &mut crate::Environment,
    _cls: Class,
    _sel: SEL,
    imp: IMP,
) -> ConstVoidPtr {
    match imp {
        IMP::Guest(gf) => gf.to_ptr(),
        IMP::Host(host_imp) => {
            // The token must be callable: swizzling code commonly stores the
            // original IMP and later calls it directly (Firebase Analytics'
            // screen tracking does this for -[UIViewController
            // viewDidAppear:]). So it is a real SVC stub that calls the host
            // implementation with the caller's registers, just like a message
            // send would. Tokens are keyed by the implementation itself rather
            // than by (class, selector), because a stub keeps calling the host
            // function it was made for even after the method is swizzled.
            let key = host_imp as *const dyn HostIMP as *const () as usize;
            let token = if let Some(&t) = env.objc.host_imp_tokens.get(&key) {
                t
            } else {
                let stub = env
                    .dyld
                    .create_guest_function(&mut env.mem, "[host IMP]", host_imp);
                env.cpu
                    .invalidate_cache_range(stub.addr_without_thumb_bit(), 8);
                let t = MutVoidPtr::from_bits(stub.addr_with_thumb_bit());
                env.objc.host_imp_tokens.insert(key, t);
                t
            };
            env.objc.imp_tokens.insert(token.to_bits(), imp);
            token.cast_const()
        }
    }
}

/// Decode a `Method` handle back into its (defining class, selector) pair.
/// Returns `None` if the pointer is NULL or was not produced by
/// [method_handle_for] (guards against the guest passing an arbitrary
/// pointer).
fn method_handle_decode(env: &crate::Environment, m: ConstVoidPtr) -> Option<(Class, SEL)> {
    if m.is_null() {
        return None;
    }
    let known = env
        .objc
        .method_handles
        .values()
        .any(|p| p.to_bits() == m.to_bits());
    if !known {
        return None;
    }
    let base: ConstPtr<u32> = m.cast();
    let cls: Class = Ptr::from_bits(env.mem.read(base));
    let sel: SEL = SEL::from_bits(env.mem.read(base + 1));
    Some((cls, sel))
}

/// Get-or-create the stable opaque `Method` handle for a (defining class,
/// selector) pair.
fn method_handle_for(
    env: &mut crate::Environment,
    defining_class: Class,
    sel: SEL,
) -> ConstVoidPtr {
    if let Some(&existing) = env.objc.method_handles.get(&(defining_class, sel)) {
        return existing.cast_const();
    }
    let handle: crate::mem::MutPtr<u32> = env.mem.alloc(8).cast();
    env.mem.write(handle, defining_class.to_bits());
    env.mem.write(handle + 1, sel.to_bits());
    let vp: MutVoidPtr = handle.cast();
    env.objc.method_handles.insert((defining_class, sel), vp);
    vp.cast_const()
}

/// The method's type-encoding string as host bytes, if one is recorded.
fn method_type_encoding_bytes(env: &mut crate::Environment, m: ConstVoidPtr) -> Option<Vec<u8>> {
    let types = method_getTypeEncoding(env, m);
    if types.is_null() {
        return None;
    }
    Some(env.mem.cstr_at(types.cast()).to_vec())
}

/// `struct objc_method_description`, as returned by
/// [protocol_getMethodDescription].
#[allow(non_camel_case_types)]
#[derive(Debug)]
#[repr(C, packed)]
pub struct objc_method_description {
    name: SEL,
    types: ConstPtr<u8>,
}

unsafe impl crate::mem::SafeRead for objc_method_description {}
crate::abi::impl_GuestRet_for_large_struct!(objc_method_description);

fn protocol_conforms_to_protocol(
    mem: &Mem,
    proto: id,
    other: id,
    depth: u32,
    visited: &mut Vec<id>,
) -> bool {
    if proto.is_null() || other.is_null() || depth > 16 {
        return false;
    }
    if proto == other {
        return true;
    }
    if visited.contains(&proto) {
        return false;
    }
    visited.push(proto);
    let words: ConstPtr<u32> = Ptr::from_bits(proto.to_bits());
    let adopted_list: ConstVoidPtr = Ptr::from_bits(mem.read(words + 2));
    let conforms = protocol_list_entries(mem, adopted_list)
        .into_iter()
        .any(|adopted| protocol_conforms_to_protocol(mem, adopted, other, depth + 1, visited));
    visited.pop();
    conforms
}

/// Reads the `name` field from an app's 32-bit `protocol_t` metadata.
fn protocol_get_name(mem: &Mem, protocol: id) -> ConstPtr<u8> {
    if protocol.is_null() {
        return Ptr::null();
    }
    let words: ConstPtr<u32> = Ptr::from_bits(protocol.to_bits());
    Ptr::from_bits(mem.read(words + 1))
}

/// Install `imp` for `sel` directly on `cls` (no inheritance walk).
fn set_imp_at(env: &mut crate::Environment, cls: Class, sel: SEL, imp: IMP) {
    let class_obj = env.objc.borrow_mut::<ClassHostObject>(cls);
    class_obj.methods.insert(sel, imp);
}

/// `strncpy`-style copy of `bytes` into the guest buffer `dst`, as
/// `method_getReturnType` / `method_getArgumentType` specify: the remainder
/// of the buffer is zero-filled and the result is not NUL-terminated if
/// `bytes` doesn't fit.
fn strncpy_type_string_to_guest(
    env: &mut crate::Environment,
    bytes: &[u8],
    dst: MutVoidPtr,
    dst_len: GuestUSize,
) {
    if dst.is_null() || dst_len == 0 {
        return;
    }
    let buf = env.mem.bytes_at_mut(dst.cast(), dst_len);
    let n = bytes.len().min(buf.len());
    buf[..n].copy_from_slice(&bytes[..n]);
    buf[n..].fill(0);
}

fn protocol_list_entries(mem: &Mem, protocol_list: ConstVoidPtr) -> Vec<id> {
    if protocol_list.is_null() {
        return Vec::new();
    }
    let list: ConstPtr<u32> = protocol_list.cast();
    let count = mem.read(list).min(256);
    (0..count)
        .map(|index| Ptr::from_bits(mem.read(list + 1 + index)))
        .filter(|protocol: &id| !protocol.is_null())
        .collect()
}

pub fn objc_release(env: &mut crate::Environment, obj: id) -> id {
    if !obj.is_null() {
        crate::objc::release(env, obj);
    }
    obj
}
