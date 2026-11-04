/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UITabBarController` and `UITabBar`.

use crate::frameworks::core_graphics::cg_image::{self, CGImageRef, CGImageRelease};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::NSUInteger;
use crate::frameworks::uikit::ui_font::UITextAlignmentCenter;
use crate::frameworks::uikit::ui_view::UIViewHostObject;
use crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject;
use crate::image::Image;
use crate::objc::{
    autorelease, id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil, objc_classes,
    release, retain, ClassExports, NSZonePtr,
};
use crate::Environment;

/// Height of a `UITabBar` on iPhone OS, in points.
const TAB_BAR_HEIGHT: CGFloat = 49.0;

// MARK: - UITabBar host object

#[derive(Default)]
struct UITabBarHostObject {
    /// Embedded `UIView` host state. `UITabBar` is a `UIView` subclass, so it
    /// must carry the superclass host object, otherwise every `UIView` method
    /// (frame, subviews, drawing, ...) sent to a tab bar fails to borrow.
    superclass: UIViewHostObject,
    /// `NSArray*` of `UITabBarItem*`
    items: id,
    /// Currently selected `UITabBarItem*` (weak — owned by `items`)
    selected_item: id,
    delegate: id,
    bar_tint_color: id, // UIColor*
    tint_color: id,     // UIColor*
    /// `UIImage*` set via `-setBackgroundImage:` (iOS 5+)
    background_image: id,
    /// `UIImage*` set via `-setShadowImage:` (iOS 5+)
    shadow_image: id,
    /// `UIImage*` set via `-setSelectionIndicatorImage:` (iOS 5+)
    selection_indicator_image: id,
    translucent: bool,
    /// Subviews created by [rebuild_tab_bar_item_views] to draw the items.
    item_views: Vec<id>,
    /// Bounds size the item views were last built for.
    item_views_size: Option<(CGFloat, CGFloat)>,
}
impl_HostObject_with_superclass!(UITabBarHostObject);

// MARK: - UITabBarController host object

#[derive(Default)]
struct UITabBarControllerHostObject {
    superclass: UIViewControllerHostObject,
    /// `NSArray*` of `UIViewController*`
    view_controllers: id,
    selected_index: NSUInteger,
    delegate: id,
    /// The managed `UITabBar*`
    tab_bar: id,
    more_navigation_controller: id,
    /// The child view controller whose view is currently in the container.
    displayed_view_controller: id,
}
impl_HostObject_with_superclass!(UITabBarControllerHostObject);

fn rect(x: CGFloat, y: CGFloat, width: CGFloat, height: CGFloat) -> CGRect {
    CGRect {
        origin: CGPoint { x, y },
        size: CGSize { width, height },
    }
}

/// Tab bar item images are templates: iPhone OS only uses their alpha channel
/// and fills it with a fixed colour (grey, or blue when selected). Build such
/// a filled copy of `image`.
fn tinted_template_image(env: &mut Environment, image: id, rgb: (f32, f32, f32)) -> id {
    let cg: CGImageRef = msg![env; image CGImage];
    if cg == nil {
        return nil;
    }
    let source = cg_image::borrow_image(&env.objc, cg);
    let (width, height) = source.dimensions();
    let mut pixels = source.pixels().to_vec();
    for px in pixels.as_chunks_mut::<4>().0 {
        // Pixels are premultiplied.
        let a = px[3] as f32;
        px[0] = (rgb.0 * a).round() as u8;
        px[1] = (rgb.1 * a).round() as u8;
        px[2] = (rgb.2 * a).round() as u8;
    }
    let new_cg = cg_image::from_image(env, Image::from_pixels(width, height, pixels));
    let scale: CGFloat = msg![env; image scale];
    let new_image: id = msg_class![env; UIImage imageWithCGImage:new_cg scale:scale];
    CGImageRelease(env, new_cg);
    new_image
}

/// Recreate the subviews that draw a tab bar's background and items. The real
/// UIKit draws these itself; touchHLE builds them from ordinary views.
fn rebuild_tab_bar_item_views(env: &mut Environment, tab_bar: id) {
    let old_views = std::mem::take(
        &mut env
            .objc
            .borrow_mut::<UITabBarHostObject>(tab_bar)
            .item_views,
    );
    for view in old_views {
        let _: () = msg![env; view removeFromSuperview];
        // Not released right away: this can run during a layout pass that is
        // still iterating over the old subviews.
        autorelease(env, view);
    }

    let bounds: CGRect = msg![env; tab_bar bounds];
    let (width, height) = (bounds.size.width, bounds.size.height);
    env.objc
        .borrow_mut::<UITabBarHostObject>(tab_bar)
        .item_views_size = Some((width, height));
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let mut new_views = Vec::new();

    // iPhone OS 3/4 style: a black bar with a slightly lighter top half. The
    // bar draws this itself whatever its backgroundColor is (nibs often
    // archive a clear one).
    let black: id = msg_class![env; UIColor blackColor];
    let _: () = msg![env; tab_bar setBackgroundColor:black];
    let top_half: id = msg_class![env; UIView alloc];
    let top_half: id = msg![env; top_half initWithFrame:(rect(0.0, 0.0, width, height / 2.0))];
    let top_color: id = msg_class![env; UIColor colorWithRed:(0.16f32) green:(0.16f32) blue:(0.16f32) alpha:(1.0f32)];
    let _: () = msg![env; top_half setBackgroundColor:top_color];
    let _: () = msg![env; top_half setUserInteractionEnabled:false];
    let _: () = msg![env; tab_bar addSubview:top_half];
    new_views.push(top_half);

    let (items, selected) = {
        let host = env.objc.borrow::<UITabBarHostObject>(tab_bar);
        (host.items, host.selected_item)
    };
    let count: NSUInteger = if items == nil {
        0
    } else {
        msg![env; items count]
    };
    let item_width = if count > 0 {
        width / count as CGFloat
    } else {
        width
    };

    let clear: id = msg_class![env; UIColor clearColor];
    let font: id = msg_class![env; UIFont boldSystemFontOfSize:(10.0f32)];
    for i in 0..count {
        let item: id = msg![env; items objectAtIndex:i];
        let is_selected = item == selected;
        let x = i as CGFloat * item_width;

        if is_selected {
            let highlight: id = msg_class![env; UIView alloc];
            let highlight: id = msg![env; highlight initWithFrame:(rect(x + 2.0, 2.0, item_width - 4.0, height - 4.0))];
            let color: id = msg_class![env; UIColor colorWithRed:(1.0f32) green:(1.0f32) blue:(1.0f32) alpha:(0.15f32)];
            let _: () = msg![env; highlight setBackgroundColor:color];
            let _: () = msg![env; highlight setUserInteractionEnabled:false];
            let _: () = msg![env; tab_bar addSubview:highlight];
            new_views.push(highlight);
        }

        let image: id = msg![env; item image];
        if image != nil {
            let rgb = if is_selected {
                (0.25, 0.63, 1.0)
            } else {
                (0.6, 0.6, 0.6)
            };
            let tinted = tinted_template_image(env, image, rgb);
            if tinted != nil {
                let size: CGSize = msg![env; tinted size];
                let image_view: id = msg_class![env; UIImageView alloc];
                let image_view: id = msg![env; image_view initWithImage:tinted];
                let frame = rect(
                    (x + (item_width - size.width) / 2.0).round(),
                    4.0,
                    size.width,
                    size.height,
                );
                let _: () = msg![env; image_view setFrame:frame];
                let _: () = msg![env; image_view setUserInteractionEnabled:false];
                let _: () = msg![env; tab_bar addSubview:image_view];
                new_views.push(image_view);
            }
        }

        let title: id = msg![env; item title];
        if title != nil {
            let label: id = msg_class![env; UILabel alloc];
            let label: id = msg![env; label initWithFrame:(rect(x + 2.0, height - 14.0, item_width - 4.0, 12.0))];
            let _: () = msg![env; label setText:title];
            let _: () = msg![env; label setFont:font];
            let _: () = msg![env; label setTextAlignment:UITextAlignmentCenter];
            let _: () = msg![env; label setBackgroundColor:clear];
            let _: () = msg![env; label setAdjustsFontSizeToFitWidth:true];
            let _: () = msg![env; label setMinimumFontSize:(6.0f32)];
            let text_color: id = if is_selected {
                msg_class![env; UIColor whiteColor]
            } else {
                msg_class![env; UIColor colorWithRed:(0.6f32) green:(0.6f32) blue:(0.6f32) alpha:(1.0f32)]
            };
            let _: () = msg![env; label setTextColor:text_color];
            let _: () = msg![env; label setUserInteractionEnabled:false];
            let _: () = msg![env; tab_bar addSubview:label];
            new_views.push(label);
        }
    }

    env.objc
        .borrow_mut::<UITabBarHostObject>(tab_bar)
        .item_views = new_views;
}

/// Put the selected view controller's view into the tab bar controller's
/// container view, above the tab bar's area, replacing the previous one.
fn show_selected_view_controller(env: &mut Environment, this: id) {
    let loaded: bool = msg![env; this isViewLoaded];
    if !loaded {
        return;
    }
    let container: id = msg![env; this view];
    let selected: id = msg![env; this selectedViewController];
    let (displayed, tab_bar) = {
        let host = env.objc.borrow::<UITabBarControllerHostObject>(this);
        (host.displayed_view_controller, host.tab_bar)
    };

    let bounds: CGRect = msg![env; container bounds];
    let tab_bar_hidden: bool = tab_bar == nil || msg![env; tab_bar isHidden];
    let content_height = if tab_bar_hidden {
        bounds.size.height
    } else {
        bounds.size.height - TAB_BAR_HEIGHT
    };
    if tab_bar != nil {
        let tab_bar_frame = rect(0.0, content_height, bounds.size.width, TAB_BAR_HEIGHT);
        let _: () = msg![env; tab_bar setFrame:tab_bar_frame];
        rebuild_tab_bar_item_views(env, tab_bar);
    }

    if displayed == selected {
        if selected != nil {
            let view: id = msg![env; selected view];
            let _: () =
                msg![env; view setFrame:(rect(0.0, 0.0, bounds.size.width, content_height))];
        }
        return;
    }

    if displayed != nil {
        let displayed_loaded: bool = msg![env; displayed isViewLoaded];
        if displayed_loaded {
            let old_view: id = msg![env; displayed view];
            let _: () = msg![env; displayed viewWillDisappear:false];
            let _: () = msg![env; old_view removeFromSuperview];
            let _: () = msg![env; displayed viewDidDisappear:false];
        }
    }

    if selected != nil {
        let view: id = msg![env; selected view];
        if view != nil {
            let _: () =
                msg![env; view setFrame:(rect(0.0, 0.0, bounds.size.width, content_height))];
            let _: () = msg![env; selected viewWillAppear:false];
            if tab_bar != nil {
                let _: () = msg![env; container insertSubview:view belowSubview:tab_bar];
            } else {
                let _: () = msg![env; container addSubview:view];
            }
            let _: () = msg![env; selected viewDidAppear:false];
        }
    }
    env.objc
        .borrow_mut::<UITabBarControllerHostObject>(this)
        .displayed_view_controller = selected;
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// MARK: - UITabBar
// =========================================================================

@implementation UITabBar: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UITabBarHostObject {
        translucent: true,
        ..Default::default()
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    let this: id = msg_super![env; this init];
    let items = msg_class![env; NSArray new];
    env.objc.borrow_mut::<UITabBarHostObject>(this).items = items;
    this
}

- (id)initWithFrame:(CGRect)frame {
    let this: id = msg_super![env; this initWithFrame:frame];
    let items = msg_class![env; NSArray new];
    env.objc.borrow_mut::<UITabBarHostObject>(this).items = items;
    this
}

- (id)initWithCoder:(id)coder {
    let this: id = msg_super![env; this initWithCoder:coder];

    let items_key = get_static_str(env, "UIItems");
    let mut items: id = msg![env; coder decodeObjectForKey:items_key];
    if items == nil {
        items = msg_class![env; NSArray new];
    } else {
        retain(env, items);
    }
    env.objc.borrow_mut::<UITabBarHostObject>(this).items = items;

    // "UIDelegate" is deliberately not decoded: it is normally the owning
    // UITabBarController, which is still being decoded at this point (the
    // unarchiver would recurse into it again), and which sets itself as the
    // delegate once it has decoded the tab bar.

    this
}

- (())dealloc {
    let host = env.objc.borrow_mut::<UITabBarHostObject>(this);
    let item_views = std::mem::take(&mut host.item_views);
    let (items, bar_tint_color, tint_color, background_image, shadow_image, selection_indicator_image) = (
        host.items,
        host.bar_tint_color,
        host.tint_color,
        host.background_image,
        host.shadow_image,
        host.selection_indicator_image,
    );
    for view in item_views {
        release(env, view);
    }
    release(env, items);
    release(env, bar_tint_color);
    release(env, tint_color);
    release(env, background_image);
    release(env, shadow_image);
    release(env, selection_indicator_image);
    msg_super![env; this dealloc]
}

- (())layoutSubviews {
    let bounds: CGRect = msg![env; this bounds];
    let built_for = env.objc.borrow::<UITabBarHostObject>(this).item_views_size;
    if built_for != Some((bounds.size.width, bounds.size.height)) {
        rebuild_tab_bar_item_views(env, this);
    }
}

// MARK: Items

- (id)items { // NSArray* of UITabBarItem*
    env.objc.borrow::<UITabBarHostObject>(this).items
}

- (())setItems:(id)items { // NSArray*
    let old = env.objc.borrow::<UITabBarHostObject>(this).items;
    retain(env, items);
    release(env, old);
    env.objc.borrow_mut::<UITabBarHostObject>(this).items = items;
    // Clear selected item — caller must set it again if desired.
    env.objc.borrow_mut::<UITabBarHostObject>(this).selected_item = nil;
    rebuild_tab_bar_item_views(env, this);
}

- (())setItems:(id)items animated:(bool)_animated {
    let _: () = msg![env; this setItems:items];
}

// MARK: Selection

- (id)selectedItem { // UITabBarItem*
    env.objc.borrow::<UITabBarHostObject>(this).selected_item
}

// Like UIKit, this only changes the highlighted item; it doesn't notify the
// delegate (that only happens when the user taps an item).
- (())setSelectedItem:(id)item {
    let (items, old_selected) = {
        let host = env.objc.borrow::<UITabBarHostObject>(this);
        (host.items, host.selected_item)
    };

    if item == old_selected { return; }

    let mut found = false;
    if items != nil {
        let count: NSUInteger = msg![env; items count];
        for i in 0..count {
            let obj: id = msg![env; items objectAtIndex:i];
            if obj == item {
                found = true;
                break;
            }
        }
    }

    // nil deselects all items.
    if found || item == nil {
        env.objc.borrow_mut::<UITabBarHostObject>(this).selected_item = item;
        rebuild_tab_bar_item_views(env, this);
    } else {
        log!("Warning: [UITabBar setSelectedItem:] item {:?} not found in items array.", item);
    }
}

// MARK: Touches

- (())touchesBegan:(id)_touches withEvent:(id)_event {
}

- (())touchesMoved:(id)_touches withEvent:(id)_event {
}

- (())touchesEnded:(id)touches withEvent:(id)_event {
    let items = env.objc.borrow::<UITabBarHostObject>(this).items;
    let count: NSUInteger = if items == nil { 0 } else { msg![env; items count] };
    if count == 0 {
        return;
    }
    let touch: id = msg![env; touches anyObject];
    let location: CGPoint = msg![env; touch locationInView:this];
    let bounds: CGRect = msg![env; this bounds];
    if location.x < 0.0
        || location.y < 0.0
        || location.x >= bounds.size.width
        || location.y >= bounds.size.height
    {
        return;
    }
    let index = ((location.x / (bounds.size.width / count as CGFloat)) as NSUInteger).min(count - 1);
    let item: id = msg![env; items objectAtIndex:index];
    let enabled: bool = msg![env; item isEnabled];
    if !enabled {
        return;
    }
    let _: () = msg![env; this setSelectedItem:item];
    let delegate = env.objc.borrow::<UITabBarHostObject>(this).delegate;
    if delegate != nil {
        let sel = env
            .objc
            .register_host_selector("tabBar:didSelectItem:".to_string(), &mut env.mem);
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let _: () = msg![env; delegate tabBar:this didSelectItem:item];
        }
    }
}

- (())touchesCancelled:(id)_touches withEvent:(id)_event {
}

// MARK: Delegate

- (id)delegate {
    env.objc.borrow::<UITabBarHostObject>(this).delegate
}

// The delegate is a weak reference, as in UIKit.
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<UITabBarHostObject>(this).delegate = delegate;
}

// MARK: Appearance

- (id)barTintColor { // UIColor*
    env.objc.borrow::<UITabBarHostObject>(this).bar_tint_color
}

- (())setBarTintColor:(id)color { // UIColor*
    let old = env.objc.borrow::<UITabBarHostObject>(this).bar_tint_color;
    retain(env, color);
    release(env, old);
    env.objc.borrow_mut::<UITabBarHostObject>(this).bar_tint_color = color;
}

- (id)tintColor { // UIColor*
    env.objc.borrow::<UITabBarHostObject>(this).tint_color
}

- (())setTintColor:(id)color { // UIColor*
    let old = env.objc.borrow::<UITabBarHostObject>(this).tint_color;
    retain(env, color);
    release(env, old);
    env.objc.borrow_mut::<UITabBarHostObject>(this).tint_color = color;
}

- (bool)isTranslucent {
    env.objc.borrow::<UITabBarHostObject>(this).translucent
}

- (())setTranslucent:(bool)translucent {
    env.objc.borrow_mut::<UITabBarHostObject>(this).translucent = translucent;
}

// MARK: Background / shadow image stubs

- (id)backgroundImage { // UIImage*
    env.objc.borrow::<UITabBarHostObject>(this).background_image
}

// Per Apple's documentation the setter is per-state (UIControlState); this
// simplified bar stores one image and uses it for every state, which matches
// how apps actually use it (a single background image for the whole bar).
- (())setBackgroundImage:(id)image {
    let old = env.objc.borrow::<UITabBarHostObject>(this).background_image;
    retain(env, image);
    release(env, old);
    env.objc.borrow_mut::<UITabBarHostObject>(this).background_image = image;
}

- (id)shadowImage { // UIImage*
    env.objc.borrow::<UITabBarHostObject>(this).shadow_image
}

- (())setShadowImage:(id)image {
    let old = env.objc.borrow::<UITabBarHostObject>(this).shadow_image;
    retain(env, image);
    release(env, old);
    env.objc.borrow_mut::<UITabBarHostObject>(this).shadow_image = image;
}

- (id)selectionIndicatorImage { // UIImage*
    env.objc.borrow::<UITabBarHostObject>(this).selection_indicator_image
}

- (())setSelectionIndicatorImage:(id)image {
    let old = env
        .objc
        .borrow::<UITabBarHostObject>(this)
        .selection_indicator_image;
    retain(env, image);
    release(env, old);
    env.objc
        .borrow_mut::<UITabBarHostObject>(this)
        .selection_indicator_image = image;
}

// MARK: Custom items editing

- (())beginCustomizingItems:(id)_items {
    log!("TODO: [UITabBar beginCustomizingItems:] — ignored");
}

- (bool)endCustomizingAnimated:(bool)_animated {
    log!("TODO: [UITabBar endCustomizingAnimated:] — returning NO");
    false
}

- (bool)isCustomizing {
    false
}

@end

// =========================================================================
// MARK: - UITabBarController
// =========================================================================

@implementation UITabBarController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UITabBarControllerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    let view_controllers = msg_class![env; NSArray new];
    // Create the owned UITabBar instance.
    let tab_bar: id = msg_class![env; UITabBar new];
    let _: () = msg![env; tab_bar setDelegate:this];
    {
        let host = env.objc.borrow_mut::<UITabBarControllerHostObject>(this);
        host.view_controllers = view_controllers;
        host.tab_bar = tab_bar;
    }
    this
}

- (id)initWithCoder:(id)coder {
    let this: id = msg_super![env; this initWithCoder:coder];

    // A UITabBarController archived in a nib stores its tab bar under
    // "UITabBar" and its view controllers under "UIViewControllers".
    // UIViewController's -initWithCoder: doesn't know about these keys;
    // without them the controller has no tabs and its view stays empty.
    let tab_bar_key = get_static_str(env, "UITabBar");
    let mut tab_bar: id = msg![env; coder decodeObjectForKey:tab_bar_key];
    if tab_bar == nil {
        tab_bar = msg_class![env; UITabBar new];
    } else {
        retain(env, tab_bar);
    }
    let _: () = msg![env; tab_bar setDelegate:this];
    let empty: id = msg_class![env; NSArray new];
    {
        let host = env.objc.borrow_mut::<UITabBarControllerHostObject>(this);
        host.tab_bar = tab_bar;
        host.view_controllers = empty;
    }

    let vcs_key = get_static_str(env, "UIViewControllers");
    let view_controllers: id = msg![env; coder decodeObjectForKey:vcs_key];
    if view_controllers != nil {
        let _: () = msg![env; this setViewControllers:view_controllers animated:false];
    }

    this
}

// MARK: - More Navigation Controller

- (id)moreNavigationController {
    let current_nav = env.objc.borrow::<UITabBarControllerHostObject>(this).more_navigation_controller;

    if current_nav != nil {
        return current_nav;
    }

    let nav_controller: id = msg_class![env; UINavigationController alloc];
    let nav_controller: id = msg![env; nav_controller init];

    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).more_navigation_controller = nav_controller;

    nav_controller
}

- (())dealloc {
    let host = env.objc.borrow::<UITabBarControllerHostObject>(this);
    let (view_controllers, delegate, tab_bar, more_nav) =
        (host.view_controllers, host.delegate, host.tab_bar, host.more_navigation_controller);
    if tab_bar != nil {
        let _: () = msg![env; tab_bar setDelegate:nil];
    }
    release(env, view_controllers);
    release(env, delegate);
    release(env, tab_bar);
    release(env, more_nav);
    msg_super![env; this dealloc]
}

// MARK: - View

// The tab bar controller's view is a container holding the selected view
// controller's view and, below it, the tab bar.
- (())loadView {
    let screen: id = msg_class![env; UIScreen mainScreen];
    let frame: CGRect = msg![env; screen applicationFrame];
    let view: id = msg_class![env; UIView alloc];
    let view: id = msg![env; view initWithFrame:frame];
    // UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight
    let _: () = msg![env; view setAutoresizingMask:(18u32)];
    let _: () = msg![env; this setView:view];
    release(env, view);

    let tab_bar = env.objc.borrow::<UITabBarControllerHostObject>(this).tab_bar;
    if tab_bar != nil {
        // UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleTopMargin
        let _: () = msg![env; tab_bar setAutoresizingMask:(10u32)];
        let _: () = msg![env; view addSubview:tab_bar];
    }
    show_selected_view_controller(env, this);
}

// MARK: - View controllers

- (id)viewControllers {
    env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers
}

- (())setViewControllers:(id)vcs {
    let _: () = msg![env; this setViewControllers:vcs animated:false];
}

- (())setViewControllers:(id)vcs animated:(bool)animated {
    let old_vcs = env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers;
    retain(env, vcs);
    release(env, old_vcs);
    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).view_controllers = vcs;

    // Give the tab bar the items of the view controllers.
    let tab_bar = env.objc.borrow::<UITabBarControllerHostObject>(this).tab_bar;
    if tab_bar != nil && vcs != nil {
        let items_array: id = msg_class![env; NSMutableArray array];
        let count: NSUInteger = msg![env; vcs count];

        for i in 0..count {
            let vc: id = msg![env; vcs objectAtIndex:i];
            let item: id = msg![env; vc tabBarItem];
            if item != nil {
                let _: () = msg![env; items_array addObject:item];
            }
        }

        let _: () = msg![env; tab_bar setItems:items_array animated:animated];
    }

    // Force re-selection, so that the tab bar highlights the item and the
    // container shows the right view.
    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).selected_index = NSUInteger::MAX;
    let _: () = msg![env; this setSelectedIndex:0u32];
}

// MARK: - Selected index / controller

- (NSUInteger)selectedIndex {
    let index = env.objc.borrow::<UITabBarControllerHostObject>(this).selected_index;
    if index == NSUInteger::MAX { 0 } else { index }
}

- (())setSelectedIndex:(NSUInteger)index {
    let (vcs, tab_bar) = {
        let host = env.objc.borrow::<UITabBarControllerHostObject>(this);
        (host.view_controllers, host.tab_bar)
    };

    if vcs == nil { return; }
    let count: NSUInteger = msg![env; vcs count];
    if index >= count { return; }

    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).selected_index = index;

    if tab_bar != nil {
        let vc: id = msg![env; vcs objectAtIndex:index];
        let item: id = msg![env; vc tabBarItem];
        let _: () = msg![env; tab_bar setSelectedItem:item];
    }
    show_selected_view_controller(env, this);
}

- (id)selectedViewController {
    let vcs = env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers;
    if vcs == nil {
        return nil;
    }
    let count: NSUInteger = msg![env; vcs count];
    if count == 0 {
        return nil;
    }
    let idx: NSUInteger = msg![env; this selectedIndex];
    if idx >= count {
        return nil;
    }
    msg![env; vcs objectAtIndex:idx]
}

- (())setSelectedViewController:(id)view_controller {
    let vcs = env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers;
    let count: NSUInteger = if vcs == nil { 0 } else { msg![env; vcs count] };
    let mut found_index: Option<NSUInteger> = None;
    for i in 0..count {
        let vc: id = msg![env; vcs objectAtIndex:i];
        if vc == view_controller {
            found_index = Some(i);
            break;
        }
    }
    if let Some(index) = found_index {
        let _: () = msg![env; this setSelectedIndex:index];
    } else {
        log!("Warning: [UITabBarController setSelectedViewController:] view controller not found in list");
    }
}

// MARK: - UITabBarDelegate

// Called by the tab bar when the user taps an item.
- (())tabBar:(id)_tab_bar didSelectItem:(id)item {
    let vcs = env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers;
    let count: NSUInteger = if vcs == nil { 0 } else { msg![env; vcs count] };
    let mut found: Option<(NSUInteger, id)> = None;
    for i in 0..count {
        let vc: id = msg![env; vcs objectAtIndex:i];
        let vc_item: id = msg![env; vc tabBarItem];
        if vc_item == item {
            found = Some((i, vc));
            break;
        }
    }
    let Some((index, vc)) = found else {
        return;
    };

    let delegate = env.objc.borrow::<UITabBarControllerHostObject>(this).delegate;
    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "tabBarController:shouldSelectViewController:".to_string(),
            &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let should: bool = msg![env; delegate tabBarController:this shouldSelectViewController:vc];
            if !should {
                // Restore the highlight of the current tab.
                let current: id = msg![env; this selectedViewController];
                let tab_bar = env.objc.borrow::<UITabBarControllerHostObject>(this).tab_bar;
                if current != nil && tab_bar != nil {
                    let current_item: id = msg![env; current tabBarItem];
                    let _: () = msg![env; tab_bar setSelectedItem:current_item];
                }
                return;
            }
        }
    }

    let _: () = msg![env; this setSelectedIndex:index];

    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "tabBarController:didSelectViewController:".to_string(),
            &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let _: () = msg![env; delegate tabBarController:this didSelectViewController:vc];
        }
    }
}

// MARK: - Delegate

- (id)delegate {
    env.objc.borrow::<UITabBarControllerHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    let old = env.objc.borrow::<UITabBarControllerHostObject>(this).delegate;
    retain(env, delegate);
    release(env, old);
    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).delegate = delegate;
}

// MARK: - Tab bar accessor

- (id)tabBar {
    env.objc.borrow::<UITabBarControllerHostObject>(this).tab_bar
}

// MARK: - UIViewController overrides

- (())viewWillAppear:(bool)animated {
    let vc: id = msg![env; this selectedViewController];
    if vc != nil {
        let _: () = msg![env; vc viewWillAppear:animated];
    }
}

- (())viewDidAppear:(bool)animated {
    let vc: id = msg![env; this selectedViewController];
    if vc != nil {
        let _: () = msg![env; vc viewDidAppear:animated];
    }
}

- (())viewWillDisappear:(bool)animated {
    let vc: id = msg![env; this selectedViewController];
    if vc != nil {
        let _: () = msg![env; vc viewWillDisappear:animated];
    }
}

- (())viewDidDisappear:(bool)animated {
    let vc: id = msg![env; this selectedViewController];
    if vc != nil {
        let _: () = msg![env; vc viewDidDisappear:animated];
    }
}

@end

};
