//! The panel that opens above a button of the bar: values to nudge up and
//! down, and a list to choose from.
//!
//! It is a view of the player's own, on the same glass as the bar, and
//! not a menu: its rows are 44 points high for a finger, the keyboard
//! moves through them as it does through the bar, and a list longer than
//! the window scrolls.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSBorderType, NSLayoutAttribute, NSLayoutConstraint,
    NSLayoutConstraintOrientation, NSLayoutPriorityDefaultHigh, NSLayoutPriorityDefaultLow,
    NSScrollView, NSStackView, NSTextAlignment, NSTextField, NSUserInterfaceLayoutOrientation,
    NSView,
};
use objc2_foundation::{NSEdgeInsets, NSPoint, NSString};

use super::chip::{Chip, Style};
use super::controls::{label, medium, regular};
use super::layout::{add, pin, row};
use super::surface::{Material, Surface};
use super::theme;

/// A value with a button on either side of it.
pub struct Stepper {
    /// What the value is of, such as "Size".
    pub name: &'static str,
    /// The value as it stands, such as "110%".
    pub value: String,
    /// What the two buttons are, for a viewer who cannot see them.
    pub lower: &'static str,
    pub raise: &'static str,
    pub on_step: Box<dyn Fn(Step)>,
}

/// Which way a [`Stepper`] was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Down,
    Up,
}

/// One line of the list.
pub struct Choice {
    pub title: String,
    /// Whether it is the one in effect, which gets the tick.
    pub chosen: bool,
    pub on_choose: Box<dyn Fn()>,
}

pub struct Panel {
    surface: Surface,
    heading: Retained<NSTextField>,
    steppers: Retained<NSStackView>,
    scroll: Retained<NSScrollView>,
    list: Retained<NSStackView>,
    /// The buttons now in the panel, in the order they are read in.
    chips: RefCell<Vec<Retained<Chip>>>,
    /// The row of the list that is ticked, or its first.
    in_effect: RefCell<Option<Retained<Chip>>>,
    /// Holds the panel over the button it was opened from.
    over: RefCell<Option<Retained<NSLayoutConstraint>>>,
}

impl Panel {
    /// The panel, hidden until it is opened.
    pub fn new(main_thread: MainThreadMarker, material: Material) -> Self {
        let column = || {
            let stack = NSStackView::new(main_thread);
            stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            stack.setAlignment(NSLayoutAttribute::Leading);
            stack.setSpacing(0.0);
            stack
        };
        let panel = Self {
            surface: Surface::new(main_thread, material, theme::PANEL_RADIUS),
            heading: label(main_thread, theme::FONT_SIZE, medium(), &theme::secondary_text()),
            steppers: column(),
            scroll: NSScrollView::new(main_thread),
            list: column(),
            chips: RefCell::new(Vec::new()),
            in_effect: RefCell::new(None),
            over: RefCell::new(None),
        };
        panel.view().setHidden(true);
        let (scroll, list) = (&panel.scroll, &panel.list);
        scroll.setDrawsBackground(false);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        list.setTranslatesAutoresizingMaskIntoConstraints(false);
        scroll.setDocumentView(Some(list));

        let content = panel.surface.content();
        let (inset, clip) = (theme::PANEL_INSET, scroll.contentView());
        let (leading, trailing) = (content.leadingAnchor(), content.trailingAnchor());
        add(content, &panel.heading);
        add(content, &panel.steppers);
        add(content, scroll);
        // As high as its list, for as long as the window has the room.
        let fits = scroll.heightAnchor().constraintEqualToAnchor(&list.heightAnchor());
        fits.setPriority(NSLayoutPriorityDefaultLow);
        pin(&[
            content.widthAnchor().constraintEqualToConstant(theme::PANEL_WIDTH),
            panel.heading.topAnchor().constraintEqualToAnchor_constant(&content.topAnchor(), inset),
            panel
                .heading
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&leading, inset + theme::CHIP_PADDING),
            panel.steppers.topAnchor().constraintEqualToAnchor_constant(
                &panel.heading.bottomAnchor(),
                theme::PANEL_HEADING_GAP,
            ),
            panel.steppers.leadingAnchor().constraintEqualToAnchor_constant(&leading, inset),
            panel.steppers.trailingAnchor().constraintEqualToAnchor_constant(&trailing, -inset),
            scroll.topAnchor().constraintEqualToAnchor(&panel.steppers.bottomAnchor()),
            scroll.leadingAnchor().constraintEqualToAnchor_constant(&leading, inset),
            scroll.trailingAnchor().constraintEqualToAnchor_constant(&trailing, -inset),
            scroll.bottomAnchor().constraintEqualToAnchor_constant(&content.bottomAnchor(), -inset),
            fits,
            list.topAnchor().constraintEqualToAnchor(&clip.topAnchor()),
            list.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()),
            list.widthAnchor().constraintEqualToAnchor(&clip.widthAnchor()),
        ]);
        panel
    }

    pub fn view(&self) -> &NSView {
        self.surface.view()
    }

    pub fn is_shown(&self) -> bool {
        !self.view().isHidden()
    }

    pub fn hide(&self) {
        self.view().setHidden(true);
    }

    /// The buttons in the panel, in the order they are read in.
    pub fn chips(&self) -> Vec<Retained<Chip>> {
        self.chips.borrow().clone()
    }

    /// The row of the list that is ticked, or its first if none is.
    pub fn row_in_effect(&self) -> Option<Retained<Chip>> {
        self.in_effect.borrow().clone().filter(|_| self.is_shown())
    }

    /// Whether `point`, in the coordinates of the panel's parent, is on
    /// the panel while it is open.
    pub fn holds(&self, point: NSPoint) -> bool {
        let frame = self.view().frame();
        let (x, y) = (point.x - frame.origin.x, point.y - frame.origin.y);
        self.is_shown()
            && (0.0..frame.size.width).contains(&x)
            && (0.0..frame.size.height).contains(&y)
    }

    /// Opens the panel over `button` under `heading`, with `steppers` and
    /// under them the list of `choices`, scrolled to the one in effect.
    pub fn show(
        &self,
        heading: &str,
        steppers: Vec<Stepper>,
        choices: Vec<Choice>,
        button: &NSView,
    ) {
        let main_thread = self.heading.mtm();
        self.heading.setStringValue(&NSString::from_str(heading));
        self.view().setAccessibilityLabel(Some(&NSString::from_str(heading)));
        for stack in [&self.steppers, &self.list] {
            for view in stack.arrangedSubviews() {
                view.removeFromSuperview();
            }
        }

        let mut chips = Vec::new();
        for stepper in steppers {
            let line = self.stepper(main_thread, stepper, &mut chips);
            self.steppers.addArrangedSubview(&line);
            pin(&[line.widthAnchor().constraintEqualToAnchor(&self.steppers.widthAnchor())]);
        }
        let mut in_effect = None;
        for choice in choices {
            let chip = Chip::new(main_thread, Style::Row);
            chip.set_title(&choice.title);
            if choice.chosen {
                chip.set_symbol("checkmark", theme::PANEL_TICK_SIZE);
                in_effect = Some(chip.clone());
            }
            chip.on_click(choice.on_choose);
            self.list.addArrangedSubview(&chip);
            pin(&[chip.widthAnchor().constraintEqualToAnchor(&self.list.widthAnchor())]);
            chips.push(chip);
        }

        // Its right edge on the button's, unless the window ends before
        // the panel would.
        let over = self.view().trailingAnchor().constraintEqualToAnchor(&button.trailingAnchor());
        over.setPriority(NSLayoutPriorityDefaultHigh);
        if let Some(old) = self.over.replace(Some(over.clone())) {
            old.setActive(false);
        }
        over.setActive(true);
        self.view().setHidden(false);

        // A list scrolls to where it is told once it knows how long it is.
        self.view().layoutSubtreeIfNeeded();
        let in_effect = in_effect.or_else(|| self.first_row());
        if let Some(row) = &in_effect {
            row.scrollRectToVisible(row.bounds());
        }
        *self.in_effect.borrow_mut() = in_effect;
        *self.chips.borrow_mut() = chips;
    }

    fn first_row(&self) -> Option<Retained<Chip>> {
        self.list.arrangedSubviews().firstObject()?.downcast().ok()
    }

    /// A line that names a value, with the value between a button that
    /// lowers it and one that raises it.
    fn stepper(
        &self,
        main_thread: MainThreadMarker,
        stepper: Stepper,
        chips: &mut Vec<Retained<Chip>>,
    ) -> Retained<NSStackView> {
        let name = label(main_thread, theme::CONTROL_FONT_SIZE, regular(), &theme::text());
        name.setStringValue(&NSString::from_str(stepper.name));
        let value = label(main_thread, theme::CONTROL_FONT_SIZE, medium(), &theme::text());
        value.setStringValue(&NSString::from_str(&stepper.value));
        value.setAlignment(NSTextAlignment::Center);
        let on_step = Rc::new(stepper.on_step);
        let button = |symbol, name: &str, step| {
            let chip = Chip::new(main_thread, Style::Plain);
            chip.set_symbol(symbol, theme::SYMBOL_SIZE);
            chip.set_name(name);
            chip.setToolTip(Some(&NSString::from_str(name)));
            let on_step = Rc::clone(&on_step);
            chip.on_click(move || on_step(step));
            chip
        };
        let lower = button("minus", stepper.lower, Step::Down);
        let raise = button("plus", stepper.raise, Step::Up);

        let views: [&NSView; 4] = [&name, &lower, &value, &raise];
        let line = row(&views, theme::CONTROL_SPACING, main_thread);
        line.setEdgeInsets(NSEdgeInsets {
            top: 0.0,
            left: theme::CHIP_PADDING,
            bottom: 0.0,
            right: 0.0,
        });
        // The name takes the room the rest leaves, which puts the buttons
        // at the right.
        name.setContentHuggingPriority_forOrientation(
            NSLayoutPriorityDefaultLow - 1.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        pin(&[value.widthAnchor().constraintEqualToConstant(theme::PANEL_VALUE_WIDTH)]);
        chips.extend([lower, raise]);
        line
    }
}
