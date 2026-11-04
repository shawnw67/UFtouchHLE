/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! A minimal paged container view controller.

use crate::abi::{CallFromHost, GuestFunction};
use crate::frameworks::core_graphics::{CGFloat, CGRect};
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject;
use crate::mem::ConstPtr;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil, objc_classes, release,
    retain, ClassExports, NSZonePtr,
};
use crate::Environment;

pub type UIPageViewControllerTransitionStyle = NSInteger;
pub type UIPageViewControllerNavigationOrientation = NSInteger;
pub type UIPageViewControllerNavigationDirection = NSInteger;
pub type UIPageViewControllerSpineLocation = NSInteger;

pub const UI_PAGE_VIEW_CONTROLLER_TRANSITION_STYLE_PAGE_CURL: UIPageViewControllerTransitionStyle =
    0;
#[allow(dead_code)]
pub const UI_PAGE_VIEW_CONTROLLER_TRANSITION_STYLE_SCROLL: UIPageViewControllerTransitionStyle = 1;
pub const UI_PAGE_VIEW_CONTROLLER_NAVIGATION_ORIENTATION_HORIZONTAL:
    UIPageViewControllerNavigationOrientation = 0;
#[allow(dead_code)]
pub const UI_PAGE_VIEW_CONTROLLER_NAVIGATION_ORIENTATION_VERTICAL:
    UIPageViewControllerNavigationOrientation = 1;
pub const UI_PAGE_VIEW_CONTROLLER_NAVIGATION_DIRECTION_FORWARD:
    UIPageViewControllerNavigationDirection = 0;
#[allow(dead_code)]
pub const UI_PAGE_VIEW_CONTROLLER_NAVIGATION_DIRECTION_REVERSE:
    UIPageViewControllerNavigationDirection = 1;
pub const UI_PAGE_VIEW_CONTROLLER_SPINE_LOCATION_MIN: UIPageViewControllerSpineLocation = 1;
#[allow(dead_code)]
pub const UI_PAGE_VIEW_CONTROLLER_SPINE_LOCATION_MID: UIPageViewControllerSpineLocation = 2;
#[allow(dead_code)]
pub const UI_PAGE_VIEW_CONTROLLER_SPINE_LOCATION_MAX: UIPageViewControllerSpineLocation = 3;

#[derive(Default)]
struct UIPageViewControllerHostObject {
    superclass: UIViewControllerHostObject,
    data_source: id,
    delegate: id,
    view_controllers: id,
    displayed_view_controller: id,
    transition_style: UIPageViewControllerTransitionStyle,
    navigation_orientation: UIPageViewControllerNavigationOrientation,
    spine_location: UIPageViewControllerSpineLocation,
    double_sided: bool,
    inter_page_spacing: CGFloat,
}
impl_HostObject_with_superclass!(UIPageViewControllerHostObject);

fn call_completion(env: &mut Environment, completion: id) {
    if completion == nil {
        return;
    }
    let invoke_address: u32 = env
        .mem
        .read(ConstPtr::<u32>::from_bits(completion.to_bits() + 12));
    if invoke_address != 0 {
        let invoke = GuestFunction::from_addr_with_thumb_bit(invoke_address);
        let _: () = invoke.call_from_host(env, (completion, 1u32));
    }
}

fn remove_displayed_view_controller(env: &mut Environment, controller: id) {
    if controller == nil {
        return;
    }
    let _: () = msg![env; controller willMoveToParentViewController:nil];
    let view: id = msg![env; controller view];
    if view != nil {
        let _: () = msg![env; view removeFromSuperview];
    }
    let _: () = msg![env; controller removeFromParentViewController];
    let _: () = msg![env; controller didMoveToParentViewController:nil];
}

fn display_view_controller(env: &mut Environment, container: id, controller: id) {
    if controller == nil {
        return;
    }
    let _: () = msg![env; controller willMoveToParentViewController:container];
    let _: () = msg![env; container addChildViewController:controller];
    let container_view: id = msg![env; container view];
    let page_view: id = msg![env; controller view];
    if container_view != nil && page_view != nil {
        let bounds: CGRect = msg![env; container_view bounds];
        let _: () = msg![env; page_view setFrame:bounds];
        let _: () = msg![env; container_view addSubview:page_view];
    }
    let _: () = msg![env; controller didMoveToParentViewController:container];
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIPageViewController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::new(UIPageViewControllerHostObject {
        spine_location: UI_PAGE_VIEW_CONTROLLER_SPINE_LOCATION_MIN,
        ..Default::default()
    });
    env.objc.alloc_object(this, host, &mut env.mem)
}

- (id)init {
    msg![env; this initWithTransitionStyle:UI_PAGE_VIEW_CONTROLLER_TRANSITION_STYLE_PAGE_CURL
                      navigationOrientation:UI_PAGE_VIEW_CONTROLLER_NAVIGATION_ORIENTATION_HORIZONTAL
                                    options:nil]
}

- (id)initWithTransitionStyle:(UIPageViewControllerTransitionStyle)style
       navigationOrientation:(UIPageViewControllerNavigationOrientation)orientation
                     options:(id)options {
    let this: id = msg![env; this initWithNibName:nil bundle:nil];
    if this != nil {
        let spine_key = crate::frameworks::foundation::ns_string::get_static_str(
            env,
            "UIPageViewControllerOptionSpineLocationKey",
        );
        let spacing_key = crate::frameworks::foundation::ns_string::get_static_str(
            env,
            "UIPageViewControllerOptionInterPageSpacingKey",
        );
        let spine_value: id = msg![env; options objectForKey:spine_key];
        let spacing_value: id = msg![env; options objectForKey:spacing_key];
        let spine_location: Option<UIPageViewControllerSpineLocation> =
            (spine_value != nil).then(|| msg![env; spine_value integerValue]);
        let inter_page_spacing: Option<CGFloat> =
            (spacing_value != nil).then(|| msg![env; spacing_value floatValue]);
        let host = env
            .objc
            .borrow_mut::<UIPageViewControllerHostObject>(this);
        host.transition_style = style;
        host.navigation_orientation = orientation;
        if let Some(spine_location) = spine_location {
            host.spine_location = spine_location;
        }
        if let Some(inter_page_spacing) = inter_page_spacing {
            host.inter_page_spacing = inter_page_spacing;
        }
    }
    this
}

- (id)initWithCoder:(id)coder {
    let this: id = msg_super![env; this initWithCoder:coder];
    let key = crate::frameworks::foundation::ns_string::get_static_str(
        env,
        "UIPageViewControllers",
    );
    let controllers: id = msg![env; coder decodeObjectForKey:key];
    if controllers != nil {
        let count: NSUInteger = msg![env; controllers count];
        if count != 0 {
            () = msg![env; this setViewControllers:controllers
                                        direction:UI_PAGE_VIEW_CONTROLLER_NAVIGATION_DIRECTION_FORWARD
                                         animated:false
                                       completion:nil];
        }
    }
    this
}

- (id)dataSource {
    env.objc
        .borrow::<UIPageViewControllerHostObject>(this)
        .data_source
}

- (())setDataSource:(id)data_source {
    if data_source != nil {
        retain(env, data_source);
    }
    let old = std::mem::replace(
        &mut env
            .objc
            .borrow_mut::<UIPageViewControllerHostObject>(this)
            .data_source,
        data_source,
    );
    if old != nil {
        release(env, old);
    }
}

- (id)delegate {
    env.objc
        .borrow::<UIPageViewControllerHostObject>(this)
        .delegate
}

- (())setDelegate:(id)delegate {
    if delegate != nil {
        retain(env, delegate);
    }
    let old = std::mem::replace(
        &mut env
            .objc
            .borrow_mut::<UIPageViewControllerHostObject>(this)
            .delegate,
        delegate,
    );
    if old != nil {
        release(env, old);
    }
}

- (id)viewControllers {
    env.objc
        .borrow::<UIPageViewControllerHostObject>(this)
        .view_controllers
}

- (())setViewControllers:(id)controllers
               direction:(UIPageViewControllerNavigationDirection)_direction
                animated:(bool)_animated
              completion:(id)completion {
    let new_controller = if controllers == nil {
        nil
    } else {
        let count: NSUInteger = msg![env; controllers count];
        if count == 0 {
            nil
        } else {
            msg![env; controllers objectAtIndex:0]
        }
    };
    if controllers != nil {
        retain(env, controllers);
    }
    let (old_controllers, old_displayed) = {
        let host = env
            .objc
            .borrow_mut::<UIPageViewControllerHostObject>(this);
        (
            std::mem::replace(&mut host.view_controllers, controllers),
            std::mem::replace(&mut host.displayed_view_controller, new_controller),
        )
    };
    if old_displayed != new_controller {
        remove_displayed_view_controller(env, old_displayed);
        display_view_controller(env, this, new_controller);
    }
    if old_controllers != nil {
        release(env, old_controllers);
    }
    call_completion(env, completion);
}

- (UIPageViewControllerTransitionStyle)transitionStyle {
    env.objc
        .borrow::<UIPageViewControllerHostObject>(this)
        .transition_style
}

- (UIPageViewControllerNavigationOrientation)navigationOrientation {
    env.objc
        .borrow::<UIPageViewControllerHostObject>(this)
        .navigation_orientation
}

- (UIPageViewControllerSpineLocation)spineLocation {
    env.objc
        .borrow::<UIPageViewControllerHostObject>(this)
        .spine_location
}

- (CGFloat)interPageSpacing {
    env.objc
        .borrow::<UIPageViewControllerHostObject>(this)
        .inter_page_spacing
}

- (())setSpineLocation:(UIPageViewControllerSpineLocation)location {
    env.objc
        .borrow_mut::<UIPageViewControllerHostObject>(this)
        .spine_location = location;
}

- (bool)isDoubleSided {
    env.objc
        .borrow::<UIPageViewControllerHostObject>(this)
        .double_sided
}

- (())setDoubleSided:(bool)double_sided {
    env.objc
        .borrow_mut::<UIPageViewControllerHostObject>(this)
        .double_sided = double_sided;
}

- (id)gestureRecognizers {
    msg_class![env; NSArray array]
}

- (())viewDidLayoutSubviews {
    () = msg_super![env; this viewDidLayoutSubviews];
    let controller = env
        .objc
        .borrow::<UIPageViewControllerHostObject>(this)
        .displayed_view_controller;
    if controller != nil {
        let container_view: id = msg![env; this view];
        let page_view: id = msg![env; controller view];
        if container_view != nil && page_view != nil {
            let bounds: CGRect = msg![env; container_view bounds];
            () = msg![env; page_view setFrame:bounds];
        }
    }
}

- (())dealloc {
    let (view_controllers, displayed, data_source, delegate) = {
        let host = env
            .objc
            .borrow_mut::<UIPageViewControllerHostObject>(this);
        (
            std::mem::take(&mut host.view_controllers),
            std::mem::take(&mut host.displayed_view_controller),
            std::mem::take(&mut host.data_source),
            std::mem::take(&mut host.delegate),
        )
    };
    remove_displayed_view_controller(env, displayed);
    release(env, view_controllers);
    release(env, data_source);
    release(env, delegate);
    msg_super![env; this dealloc]
}

@end

};
