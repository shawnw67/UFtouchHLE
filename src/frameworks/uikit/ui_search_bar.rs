/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! UISearchBar.

use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::foundation::{ns_string, NSUInteger};
use crate::objc::{
    id, msg, msg_super, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

#[derive(Default)]
struct UISearchBarHostObject {
    delegate: id,
    /// Per-state overrides for `setSearchFieldBackgroundImage:forState:`,
    /// keyed by `UIControlState` (0 = normal). UIKit reference:
    /// per-state values take precedence over the shared background image.
    search_field_background_images: Vec<(i32, id)>,
    /// Per-icon, per-state images from `setImage:forSearchBarIcon:state:`.
    search_bar_icon_images: Vec<(i32, i32, id)>,
    /// Per-state scope bar button background images.
    scope_bar_button_background_images: Vec<(i32, id)>,
    /// Per-state-pair scope bar divider images (left state, right state).
    scope_bar_divider_images: Vec<(i32, i32, id)>,
    /// Per-state scope bar title text attributes dictionaries.
    scope_bar_title_text_attributes: Vec<(i32, id)>,
    /// Per-icon position adjustments (`UIOffset` NSValue-wrapped or NSValue).
    search_bar_icon_position_adjustments: Vec<(i32, id)>,
    text: id,
    placeholder: id,
    prompt: id,
    bar_style: i32,
    search_bar_style: i32,
    shows_cancel_button: bool,
    shows_bookmark_button: bool,
    shows_search_results_button: bool,
    search_results_button_selected: bool,

    autocorrection_type: i32,
    autocapitalization_type: i32,
    keyboard_type: i32,
    return_key_type: i32,
    spell_checking_type: i32,
    enables_return_key_automatically: bool,

    tint_color: id,
    bar_tint_color: id,
    translucent: bool,
    input_accessory_view: id,
    input_view: id,
    scope_button_titles: id,
    selected_scope_button_index: i32,
    shows_scope_bar: bool,
    background_image: id,
    scope_bar_background_image: id,

    search_field_background_position_adjustment: id,
    search_text_position_adjustment: id,
    is_first_responder: bool,
}

impl HostObject for UISearchBarHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UISearchBar: UIView

// Исправление паники "Call to class method alloc"
+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UISearchBarHostObject::default());
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    // Безопасно извлекаем свойства для
    // очистки памяти
    let (
        text, placeholder, prompt, tint_color, bar_tint_color,
        input_accessory_view, input_view, scope_button_titles,
        background_image, scope_bar_background_image,
        _field_bg_pos_adj, _search_text_pos_adj,
        field_images, icon_images, scope_button_images, divider_images,
        scope_attrs, icon_adjustments
    ) = {
        let host = env.objc.borrow::<UISearchBarHostObject>(this);
        (
            host.text, host.placeholder, host.prompt, host.tint_color, host.bar_tint_color,
            host.input_accessory_view, host.input_view, host.scope_button_titles,
            host.background_image, host.scope_bar_background_image,
            host.search_field_background_position_adjustment, host.search_text_position_adjustment,
            host.search_field_background_images.clone(),
            host.search_bar_icon_images.clone(),
            host.scope_bar_button_background_images.clone(),
            host.scope_bar_divider_images.clone(),
            host.scope_bar_title_text_attributes.clone(),
            host.search_bar_icon_position_adjustments.clone(),
        )
    };

    release(env, text);
    release(env, placeholder);
    release(env, prompt);
    release(env, tint_color);
    release(env, bar_tint_color);
    release(env, input_accessory_view);
    release(env, input_view);
    release(env, scope_button_titles);
    release(env, background_image);
    release(env, scope_bar_background_image);
    for (_, image) in field_images.iter() {
        release(env, *image);
    }
    for (_, _, image) in icon_images.iter() {
        release(env, *image);
    }
    for (_, image) in scope_button_images.iter() {
        release(env, *image);
    }
    for (_, _, image) in divider_images.iter() {
        release(env, *image);
    }
    for (_, attributes) in scope_attrs.iter() {
        release(env, *attributes);
    }
    for (_, offset) in icon_adjustments.iter() {
        release(env, *offset);
    }

    msg_super![env; this dealloc]
}

- (id)initWithFrame:(CGRect)frame {
    msg_super![env; this initWithFrame:frame]
}

// Исправление для NIB-парсера (UIClassSwapper)
- (id)initWithCoder:(id)coder {
    msg_super![env; this initWithCoder:coder]
}

- (id)delegate {
    env.objc.borrow::<UISearchBarHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    // Делегаты в UIKit обычно имеют weak-ссылку,
    // поэтому без retain/release
    env.objc.borrow_mut::<UISearchBarHostObject>(this).delegate = delegate;
}

- (id)text {
    env.objc.borrow::<UISearchBarHostObject>(this).text
}
- (())setText:(id)text {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).text;
    retain(env, text);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).text = text;
}

- (id)placeholder {
    env.objc.borrow::<UISearchBarHostObject>(this).placeholder
}
- (())setPlaceholder:(id)placeholder {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).placeholder;
    retain(env, placeholder);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).placeholder = placeholder;
}

- (id)prompt {
    env.objc.borrow::<UISearchBarHostObject>(this).prompt
}
- (())setPrompt:(id)prompt {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).prompt;
    retain(env, prompt);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).prompt = prompt;
}

- (())setBarStyle:(i32)style {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).bar_style = style;
}
- (i32)barStyle {
    env.objc.borrow::<UISearchBarHostObject>(this).bar_style
}

- (())setSearchBarStyle:(i32)style {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).search_bar_style = style;
}
- (i32)searchBarStyle {
    env.objc.borrow::<UISearchBarHostObject>(this).search_bar_style
}

- (())setShowsCancelButton:(bool)shows {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).shows_cancel_button = shows;
}
- (())setShowsCancelButton:(bool)shows animated:(bool)_animated {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).shows_cancel_button = shows;
}
- (bool)showsCancelButton {
    env.objc.borrow::<UISearchBarHostObject>(this).shows_cancel_button
}

- (())setShowsBookmarkButton:(bool)shows {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).shows_bookmark_button = shows;
}
- (bool)showsBookmarkButton {
    env.objc.borrow::<UISearchBarHostObject>(this).shows_bookmark_button
}

- (())setShowsSearchResultsButton:(bool)shows {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).shows_search_results_button = shows;
}
- (bool)showsSearchResultsButton {
    env.objc.borrow::<UISearchBarHostObject>(this).shows_search_results_button
}

- (())setSearchResultsButtonSelected:(bool)selected {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).search_results_button_selected = selected;
}
- (bool)isSearchResultsButtonSelected {
    env.objc.borrow::<UISearchBarHostObject>(this).search_results_button_selected
}

- (())setAutocorrectionType:(i32)type_val {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).autocorrection_type = type_val;
}
- (())setAutocapitalizationType:(i32)type_val {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).autocapitalization_type = type_val;
}
- (())setKeyboardType:(i32)type_val {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).keyboard_type = type_val;
}
- (())setReturnKeyType:(i32)type_val {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).return_key_type = type_val;
}
- (())setSpellCheckingType:(i32)type_val {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).spell_checking_type = type_val;
}
- (())setEnablesReturnKeyAutomatically:(bool)enables {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).enables_return_key_automatically = enables;
}

- (())setTintColor:(id)color {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).tint_color;
    retain(env, color);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).tint_color = color;
}
- (id)tintColor {
    env.objc.borrow::<UISearchBarHostObject>(this).tint_color
}

- (())setBarTintColor:(id)color {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).bar_tint_color;
    retain(env, color);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).bar_tint_color = color;
}
- (id)barTintColor {
    env.objc.borrow::<UISearchBarHostObject>(this).bar_tint_color
}

- (())setTranslucent:(bool)translucent {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).translucent = translucent;
}
- (bool)isTranslucent {
    env.objc.borrow::<UISearchBarHostObject>(this).translucent
}

- (())setInputAccessoryView:(id)view {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).input_accessory_view;
    retain(env, view);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).input_accessory_view = view;
}
- (id)inputAccessoryView {
    env.objc.borrow::<UISearchBarHostObject>(this).input_accessory_view
}

- (())setInputView:(id)view {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).input_view;
    retain(env, view);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).input_view = view;
}
- (id)inputView {
    env.objc.borrow::<UISearchBarHostObject>(this).input_view
}

- (())setScopeButtonTitles:(id)titles {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).scope_button_titles;
    retain(env, titles);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).scope_button_titles = titles;
}
- (id)scopeButtonTitles {
    env.objc.borrow::<UISearchBarHostObject>(this).scope_button_titles
}

- (())setSelectedScopeButtonIndex:(i32)index {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).selected_scope_button_index = index;
}
- (i32)selectedScopeButtonIndex {
    env.objc.borrow::<UISearchBarHostObject>(this).selected_scope_button_index
}

- (())setShowsScopeBar:(bool)shows {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).shows_scope_bar = shows;
}
- (bool)showsScopeBar {
    env.objc.borrow::<UISearchBarHostObject>(this).shows_scope_bar
}

- (())setBackgroundImage:(id)image {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).background_image;
    retain(env, image);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).background_image = image;
}
- (id)backgroundImage {
    env.objc.borrow::<UISearchBarHostObject>(this).background_image
}

- (())setScopeBarBackgroundImage:(id)image {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).scope_bar_background_image;
    retain(env, image);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).scope_bar_background_image = image;
}

- (())setSearchFieldBackgroundPositionAdjustment:(id)offset {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).search_field_background_position_adjustment;
    retain(env, offset);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).search_field_background_position_adjustment = offset;
}
- (())setSearchTextPositionAdjustment:(id)offset {
    let old = env.objc.borrow::<UISearchBarHostObject>(this).search_text_position_adjustment;
    retain(env, offset);
    release(env, old);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).search_text_position_adjustment = offset;
}

- (())touchesBegan:(id)_touches withEvent:(id)_event {
    let _: bool = msg![env; this becomeFirstResponder];
}

- (bool)canBecomeFirstResponder { true }
- (bool)canResignFirstResponder { true }

- (bool)becomeFirstResponder {
    if env.framework_state.uikit.ui_responder.first_responder == this {
        env.objc.borrow_mut::<UISearchBarHostObject>(this).is_first_responder = true;
        crate::frameworks::uikit::ui_keyboard::start_text_input(env);
        return true;
    }

    let delegate: id = msg![env; this delegate];
    let delegate_alive = search_bar_delegate_is_alive(env, delegate);
    if delegate_alive {
        let selector = env.objc.register_host_selector("searchBarShouldBeginEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:selector] && !msg![env; delegate searchBarShouldBeginEditing:this] {
            return false;
        }
    }

    let previous_responder = env.framework_state.uikit.ui_responder.first_responder;
    if previous_responder != nil && previous_responder != this && !msg![env; previous_responder resignFirstResponder] {
        return false;
    }

    crate::frameworks::uikit::ui_keyboard::post_keyboard_notifications(env, true);
    env.framework_state.uikit.ui_responder.first_responder = this;
    env.objc.borrow_mut::<UISearchBarHostObject>(this).is_first_responder = true;
    crate::frameworks::uikit::ui_keyboard::start_text_input(env);

    if delegate_alive {
        let selector = env.objc.register_host_selector("searchBarTextDidBeginEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:selector] {
            let _: () = msg![env; delegate searchBarTextDidBeginEditing:this];
        }
    }
    true
}

- (bool)resignFirstResponder {
    if env.framework_state.uikit.ui_responder.first_responder != this {
        env.objc.borrow_mut::<UISearchBarHostObject>(this).is_first_responder = false;
        return true;
    }

    let delegate: id = msg![env; this delegate];
    let delegate_alive = search_bar_delegate_is_alive(env, delegate);
    if delegate_alive {
        let selector = env.objc.register_host_selector("searchBarShouldEndEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:selector] && !msg![env; delegate searchBarShouldEndEditing:this] {
            return false;
        }
    }

    crate::frameworks::uikit::ui_keyboard::post_keyboard_notifications(env, false);
    env.framework_state.uikit.ui_responder.first_responder = nil;
    env.objc.borrow_mut::<UISearchBarHostObject>(this).is_first_responder = false;
    crate::frameworks::uikit::ui_keyboard::stop_text_input(env);

    if delegate_alive {
        let selector = env.objc.register_host_selector("searchBarTextDidEndEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:selector] {
            let _: () = msg![env; delegate searchBarTextDidEndEditing:this];
        }
    }
    true
}

- (bool)isFirstResponder {
    env.framework_state.uikit.ui_responder.first_responder == this
}

// Per the UISearchBar reference, these setters store per-state/per-icon
// values. The renderer only uses the shared background image today, but the
// values are retained and exposed through matching getters so apps that
// read back what they set (common in themed search UIs) observe round-trips.

- (())setSearchFieldBackgroundImage:(id)image forState:(i32)state {
    retain(env, image);
    let host = env.objc.borrow_mut::<UISearchBarHostObject>(this);
    host.search_field_background_images
        .push((state, image));
    log_dbg!("UISearchBar setSearchFieldBackgroundImage:{:?} forState:{:#x}", image, state);
}
- (id)searchFieldBackgroundImageForState:(i32)state {
    let host = env.objc.borrow::<UISearchBarHostObject>(this);
    host.search_field_background_images
        .iter()
        .rev()
        .find(|(s_, _)| *s_ == state)
        .map(|(_, image)| *image)
        .unwrap_or(nil)
}
- (())setImage:(id)image forSearchBarIcon:(i32)icon state:(i32)state {
    retain(env, image);
    let host = env.objc.borrow_mut::<UISearchBarHostObject>(this);
    host.search_bar_icon_images.push((icon, state, image));
    log_dbg!(
        "UISearchBar setImage:{:?} forSearchBarIcon:{:#x} state:{:#x}",
        image, icon, state
    );
}
- (id)imageForSearchBarIcon:(i32)icon state:(i32)state {
    let host = env.objc.borrow::<UISearchBarHostObject>(this);
    host.search_bar_icon_images
        .iter()
        .rev()
        .find(|(i, s, _)| *i == icon && *s == state)
        .map(|(_, _, image)| *image)
        .unwrap_or(nil)
}
- (())setScopeBarButtonBackgroundImage:(id)image forState:(i32)state {
    retain(env, image);
    env.objc
        .borrow_mut::<UISearchBarHostObject>(this)
        .scope_bar_button_background_images
        .push((state, image));
    log_dbg!(
        "UISearchBar setScopeBarButtonBackgroundImage:{:?} forState:{:#x}",
        image,
        state
    );
}
- (id)scopeBarButtonBackgroundImageForState:(i32)state {
    let host = env.objc.borrow::<UISearchBarHostObject>(this);
    host.scope_bar_button_background_images
        .iter()
        .rev()
        .find(|(s_, _)| *s_ == state)
        .map(|(_, image)| *image)
        .unwrap_or(nil)
}
- (())setScopeBarButtonDividerImage:(id)image
                  forLeftSegmentState:(i32)left
                  rightSegmentState:(i32)right {
    retain(env, image);
    let host = env.objc.borrow_mut::<UISearchBarHostObject>(this);
    host.scope_bar_divider_images.push((left, right, image));
    log_dbg!(
        "UISearchBar setScopeBarButtonDividerImage:{:?} left:{:#x} right:{:#x}",
        image,
        left,
        right
    );
}
- (())setScopeBarButtonTitleTextAttributes:(id)attributes forState:(i32)state {
    retain(env, attributes);
    let host = env.objc.borrow_mut::<UISearchBarHostObject>(this);
    host.scope_bar_title_text_attributes.push((state, attributes));
    log_dbg!(
        "UISearchBar setScopeBarButtonTitleTextAttributes:{:?} forState:{:#x}",
        attributes,
        state
    );
}
- (())setPositionAdjustment:(id)offset forSearchBarIcon:(i32)icon {
    retain(env, offset);
    let host = env.objc.borrow_mut::<UISearchBarHostObject>(this);
    host.search_bar_icon_position_adjustments.push((icon, offset));
    log_dbg!(
        "UISearchBar setPositionAdjustment:{:?} forSearchBarIcon:{:#x}",
        offset,
        icon
    );
}

@end

};

fn search_bar_delegate_is_alive(env: &mut crate::Environment, delegate: id) -> bool {
    if delegate == nil {
        return false;
    }
    let isa: u32 = env.mem.read(delegate.cast());
    isa != 0
}

#[allow(dead_code)]
fn notify_search_bar_text_changed(env: &mut crate::Environment, search_bar: id, text: id) {
    let delegate: id = msg![env; search_bar delegate];
    if !search_bar_delegate_is_alive(env, delegate) {
        return;
    }
    let selector = env
        .objc
        .register_host_selector("searchBar:textDidChange:".to_string(), &mut env.mem);
    if msg![env; delegate respondsToSelector:selector] {
        let _: () = msg![env; delegate searchBar:search_bar textDidChange:text];
    }
}

#[allow(dead_code)]
pub fn handle_text(env: &mut crate::Environment, search_bar: id, text: String) {
    let inserted = ns_string::from_rust_string(env, text);
    let current: id = msg![env; search_bar text];
    let current = if current == nil {
        ns_string::get_static_str(env, "")
    } else {
        current
    };
    let updated: id = msg![env; current stringByAppendingString:inserted];
    let _: () = msg![env; search_bar setText:updated];
    let _: () = msg![env; search_bar setNeedsDisplay];
    notify_search_bar_text_changed(env, search_bar, updated);
    release(env, updated);
    release(env, inserted);
}

#[allow(dead_code)]
pub fn handle_backspace(env: &mut crate::Environment, search_bar: id) {
    let current: id = msg![env; search_bar text];
    if current == nil {
        return;
    }
    let length: NSUInteger = msg![env; current length];
    if length == 0 {
        return;
    }
    let updated: id = msg![env; current substringToIndex:(length - 1)];
    let _: () = msg![env; search_bar setText:updated];
    let _: () = msg![env; search_bar setNeedsDisplay];
    notify_search_bar_text_changed(env, search_bar, updated);
    release(env, updated);
}

#[allow(dead_code)]
pub fn handle_return(env: &mut crate::Environment, search_bar: id) {
    let delegate: id = msg![env; search_bar delegate];
    if !search_bar_delegate_is_alive(env, delegate) {
        return;
    }
    let selector = env
        .objc
        .register_host_selector("searchBarSearchButtonClicked:".to_string(), &mut env.mem);
    if msg![env; delegate respondsToSelector:selector] {
        let _: () = msg![env; delegate searchBarSearchButtonClicked:search_bar];
    }
}
