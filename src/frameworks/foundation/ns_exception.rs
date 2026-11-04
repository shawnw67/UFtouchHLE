/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::dyld::{ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::ns_string;
use crate::mem::MutVoidPtr;
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
    SEL,
};
use crate::{export_c_func, Environment};

struct NSExceptionHostObject {
    /// `NSString*`
    name: id,
    /// `NSString*`
    reason: id,
    /// `NSDictionary*`
    user_info: id,
}
impl HostObject for NSExceptionHostObject {}

fn string_or_nil(env: &mut Environment, string: id) -> String {
    if string == nil {
        "(nil)".to_string()
    } else {
        ns_string::to_rust_string(env, string).to_string()
    }
}

// touchHLE can't unwind guest code to a @catch block, so raising an exception
// ends the app with its details. (`@throw` is handled by objc_exception_throw.)
pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSException: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSExceptionHostObject {
        name: nil,
        reason: nil,
        user_info: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)exceptionWithName:(id)name // NSString*
                 reason:(id)reason // NSString*
               userInfo:(id)user_info { // NSDictionary*
    let exception: id = msg![env; this alloc];
    let exception: id = msg![env; exception initWithName:name reason:reason userInfo:user_info];
    autorelease(env, exception)
}

+ (())raise:(id)name // NSString*
     format:(id)format, // NSString*
     ..._args {
    let name = string_or_nil(env, name);
    let format = string_or_nil(env, format);
    panic!("Objective-C exception raised: {name}: {format}");
}

- (id)initWithName:(id)name // NSString*
            reason:(id)reason // NSString*
          userInfo:(id)user_info { // NSDictionary*
    retain(env, name);
    retain(env, reason);
    retain(env, user_info);
    *env.objc.borrow_mut(this) = NSExceptionHostObject { name, reason, user_info };
    this
}

- (())dealloc {
    let &NSExceptionHostObject { name, reason, user_info } = env.objc.borrow(this);
    release(env, name);
    release(env, reason);
    release(env, user_info);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)name {
    env.objc.borrow::<NSExceptionHostObject>(this).name
}
- (id)reason {
    env.objc.borrow::<NSExceptionHostObject>(this).reason
}
- (id)userInfo {
    env.objc.borrow::<NSExceptionHostObject>(this).user_info
}
- (id)description {
    env.objc.borrow::<NSExceptionHostObject>(this).reason
}

- (())raise {
    let &NSExceptionHostObject { name, reason, .. } = env.objc.borrow(this);
    let name = string_or_nil(env, name);
    let reason = string_or_nil(env, reason);
    panic!("Objective-C exception raised: {name}: {reason}");
}

@end

// Used by NSAssert() and friends.
@implementation NSAssertionHandler: NSObject

+ (id)currentHandler {
    msg![env; this new]
}

- (())handleFailureInMethod:(SEL)selector
                     object:(id)object
                       file:(id)file // NSString*
                 lineNumber:(i32)line
                description:(id)format, // NSString*
                ..._args {
    let file = string_or_nil(env, file);
    let format = string_or_nil(env, format);
    panic!(
        "Assertion failed in -[{:?} {}], {}:{}: {}",
        object,
        selector.as_str(&env.mem),
        file,
        line,
        format
    );
}

- (())handleFailureInFunction:(id)function // NSString*
                         file:(id)file // NSString*
                   lineNumber:(i32)line
                  description:(id)format, // NSString*
                  ..._args {
    let function = string_or_nil(env, function);
    let file = string_or_nil(env, file);
    let format = string_or_nil(env, format);
    panic!("Assertion failed in {function}, {file}:{line}: {format}");
}

@end

};

// All constants are NSExceptionName
pub const CONSTANTS: ConstantExports = &[
    (
        "_NSCharacterConversionException",
        HostConstant::NSString("NSCharacterConversionException"),
    ),
    (
        "_NSDecimalNumberDivideByZeroException",
        HostConstant::NSString("NSDecimalNumberDivideByZeroException"),
    ),
    (
        "_NSDecimalNumberExactnessException",
        HostConstant::NSString("NSDecimalNumberExactnessException"),
    ),
    (
        "_NSDecimalNumberOverflowException",
        HostConstant::NSString("NSDecimalNumberOverflowException"),
    ),
    (
        "_NSDecimalNumberUnderflowException",
        HostConstant::NSString("NSDecimalNumberUnderflowException"),
    ),
    (
        "_NSDestinationInvalidException",
        HostConstant::NSString("NSDestinationInvalidException"),
    ),
    (
        "_NSFileHandleOperationException",
        HostConstant::NSString("NSFileHandleOperationException"),
    ),
    (
        "_NSGenericException",
        HostConstant::NSString("NSGenericException"),
    ),
    (
        "_NSInternalInconsistencyException",
        HostConstant::NSString("NSInternalInconsistencyException"),
    ),
    (
        "_NSInvalidArchiveOperationException",
        HostConstant::NSString("NSInvalidArchiveOperationException"),
    ),
    (
        "_NSInvalidArgumentException",
        HostConstant::NSString("NSInvalidArgumentException"),
    ),
    (
        "_NSInvalidReceivePortException",
        HostConstant::NSString("NSInvalidReceivePortException"),
    ),
    (
        "_NSInvalidSendPortException",
        HostConstant::NSString("NSInvalidSendPortException"),
    ),
    (
        "_NSInvalidUnarchiveOperationException",
        HostConstant::NSString("NSInvalidUnarchiveOperationException"),
    ),
    (
        "_NSInvocationOperationCancelledException",
        HostConstant::NSString("NSInvocationOperationCancelledException"),
    ),
    (
        "_NSInvocationOperationVoidResultException",
        HostConstant::NSString("NSInvocationOperationVoidResultException"),
    ),
    (
        "_NSMallocException",
        HostConstant::NSString("NSMallocException"),
    ),
    (
        "_NSObjectInaccessibleException",
        HostConstant::NSString("NSObjectInaccessibleException"),
    ),
    (
        "_NSObjectNotAvailableException",
        HostConstant::NSString("NSObjectNotAvailableException"),
    ),
    (
        "_NSOldStyleException",
        HostConstant::NSString("NSOldStyleException"),
    ),
    (
        "_NSParseErrorException",
        HostConstant::NSString("NSParseErrorException"),
    ),
    (
        "_NSPortReceiveException",
        HostConstant::NSString("NSPortReceiveException"),
    ),
    (
        "_NSPortSendException",
        HostConstant::NSString("NSPortSendException"),
    ),
    (
        "_NSPortTimeoutException",
        HostConstant::NSString("NSPortTimeoutException"),
    ),
    (
        "_NSRangeException",
        HostConstant::NSString("NSRangeException"),
    ),
    (
        "_NSUndefinedKeyException",
        HostConstant::NSString("NSUndefinedKeyException"),
    ),
    (
        "_NSInconsistentArchiveException",
        HostConstant::NSString("NSInconsistentArchiveException"),
    ),
    (
        "_NSPPDIncludeNotFoundException",
        HostConstant::NSString("NSPPDIncludeNotFoundException"),
    ),
    (
        "_NSPPDIncludeStackOverflowException",
        HostConstant::NSString("NSPPDIncludeStackOverflowException"),
    ),
    (
        "_NSPPDIncludeStackUnderflowException",
        HostConstant::NSString("NSPPDIncludeStackUnderflowException"),
    ),
    (
        "_NSPPDParseException",
        HostConstant::NSString("NSPPDParseException"),
    ),
    (
        "_NSRTFPropertyStackOverflowException",
        HostConstant::NSString("NSRTFPropertyStackOverflowException"),
    ),
    (
        "_NSTIFFException",
        HostConstant::NSString("NSTIFFException"),
    ),
    (
        "_NSAbortModalException",
        HostConstant::NSString("NSAbortModalException"),
    ),
    (
        "_NSAbortPrintingException",
        HostConstant::NSString("NSAbortPrintingException"),
    ),
    (
        "_NSAccessibilityException",
        HostConstant::NSString("NSAccessibilityException"),
    ),
    (
        "_NSAppKitIgnoredException",
        HostConstant::NSString("NSAppKitIgnoredException"),
    ),
    (
        "_NSAppKitVirtualMemoryException",
        HostConstant::NSString("NSAppKitVirtualMemoryException"),
    ),
    (
        "_NSBadBitmapParametersException",
        HostConstant::NSString("NSBadBitmapParametersException"),
    ),
    (
        "_NSBadComparisonException",
        HostConstant::NSString("NSBadComparisonException"),
    ),
    (
        "_NSBadRTFColorTableException",
        HostConstant::NSString("NSBadRTFColorTableException"),
    ),
    (
        "_NSBadRTFDirectiveException",
        HostConstant::NSString("NSBadRTFDirectiveException"),
    ),
    (
        "_NSBadRTFFontTableException",
        HostConstant::NSString("NSBadRTFFontTableException"),
    ),
    (
        "_NSBadRTFStyleSheetException",
        HostConstant::NSString("NSBadRTFStyleSheetException"),
    ),
    (
        "_NSBrowserIllegalDelegateException",
        HostConstant::NSString("NSBrowserIllegalDelegateException"),
    ),
    (
        "_NSColorListIOException",
        HostConstant::NSString("NSColorListIOException"),
    ),
    (
        "_NSColorListNotEditableException",
        HostConstant::NSString("NSColorListNotEditableException"),
    ),
    (
        "_NSDraggingException",
        HostConstant::NSString("NSDraggingException"),
    ),
    (
        "_NSFontUnavailableException",
        HostConstant::NSString("NSFontUnavailableException"),
    ),
    (
        "_NSIllegalSelectorException",
        HostConstant::NSString("NSIllegalSelectorException"),
    ),
    (
        "_NSImageCacheException",
        HostConstant::NSString("NSImageCacheException"),
    ),
    (
        "_NSNibLoadingException",
        HostConstant::NSString("NSNibLoadingException"),
    ),
    (
        "_NSPasteboardCommunicationException",
        HostConstant::NSString("NSPasteboardCommunicationException"),
    ),
    (
        "_NSPrintOperationExistsException",
        HostConstant::NSString("NSPrintOperationExistsException"),
    ),
    (
        "_NSPrintPackageException",
        HostConstant::NSString("NSPrintPackageException"),
    ),
    (
        "_NSPrintingCommunicationException",
        HostConstant::NSString("NSPrintingCommunicationException"),
    ),
    (
        "_NSTextLineTooLongException",
        HostConstant::NSString("NSTextLineTooLongException"),
    ),
    (
        "_NSTextNoSelectionException",
        HostConstant::NSString("NSTextNoSelectionException"),
    ),
    (
        "_NSTextReadException",
        HostConstant::NSString("NSTextReadException"),
    ),
    (
        "_NSTextWriteException",
        HostConstant::NSString("NSTextWriteException"),
    ),
    (
        "_NSTypedStreamVersionException",
        HostConstant::NSString("NSTypedStreamVersionException"),
    ),
    (
        "_NSWindowServerCommunicationException",
        HostConstant::NSString("NSWindowServerCommunicationException"),
    ),
    (
        "_NSWordTablesReadException",
        HostConstant::NSString("NSWordTablesReadException"),
    ),
    (
        "_NSWordTablesWriteException",
        HostConstant::NSString("NSWordTablesWriteException"),
    ),
    (
        "_UIViewControllerHierarchyInconsistencyException",
        HostConstant::NSString("UIViewControllerHierarchyInconsistencyException"),
    ),
    (
        "_UIApplicationInvalidInterfaceOrientationException",
        HostConstant::NSString("UIApplicationInvalidInterfaceOrientationException"),
    ),
];

/// This exception handler is supposed to do last-minute logging before the
/// program terminates. For our purposes, it's completely safe to ignore that.
fn NSSetUncaughtExceptionHandler(env: &mut Environment, handler: MutVoidPtr) {
    log!(
        "TODO: Ignoring uncaught exception handler at address {:?}",
        handler
    );
    env.framework_state
        .foundation
        .ns_exception
        .uncaught_exception_handler = handler;
}

/// Returns what was last passed to `NSSetUncaughtExceptionHandler`.
fn NSGetUncaughtExceptionHandler(env: &mut Environment) -> MutVoidPtr {
    env.framework_state
        .foundation
        .ns_exception
        .uncaught_exception_handler
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(NSSetUncaughtExceptionHandler(_)),
    export_c_func!(NSGetUncaughtExceptionHandler()),
];

#[derive(Default)]
pub struct State {
    uncaught_exception_handler: MutVoidPtr,
}
