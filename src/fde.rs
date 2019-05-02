use orbclient::{Color, Renderer};
use orbfont::{Font, Text};
use std::{char, cmp, ffi, mem, ptr, slice};
use std::ops::Try;
use std::proto::Protocol;
use uefi::Event;
use uefi::guid::Guid;
use uefi::hii::{AnimationId, ImageId, StringId};
use uefi::hii::database::HiiHandle;
use uefi::hii::ifr::{
    HiiDate, HiiRef, HiiTime, HiiValue,
    IfrOpCode, IfrOpHeader, IfrStatementHeader, IfrTypeKind, IfrTypeValue, IfrTypeValueEnum,
    IfrAction, IfrCheckbox, IfrNumeric, IfrOneOf, IfrOneOfOption, IfrOrderedList, IfrRef, IfrSubtitle
};
use uefi::status::{Error, Result, Status};
use uefi::text::TextInputKey;

use crate::display::{Display, Output, ScaledDisplay};
use crate::image::{self, Image};
use crate::key::{raw_key, Key};

// TODO: Move to uefi library {
pub const HII_STRING_PROTOCOL_GUID: Guid = Guid(0xfd96974, 0x23aa, 0x4cdc, [0xb9, 0xcb, 0x98, 0xd1, 0x77, 0x50, 0x32, 0x2a]);

#[repr(C)]
pub struct HiiStringProtocol {
    pub NewString: extern "win64" fn(), //TODO
    pub GetString: extern "win64" fn(
        &HiiStringProtocol,
        Language: *const u8,
        PackageList: HiiHandle,
        StringId: StringId,
        String: *mut u16,
        StringSize: &mut usize,
        StringFontInfo: usize, // TODO
    ) -> Status,
    pub SetString: extern "win64" fn(), //TODO
    pub GetLanguages: extern "win64" fn(), //TODO
    pub GetSecondaryLanguages: extern "win64" fn(), //TODO
}

impl HiiStringProtocol {
    pub fn string(&self, PackageList: HiiHandle, StringId: StringId) -> Result<String> {
        let mut data = vec![0u16; 4096];
        let mut len = data.len();
        (self.GetString)(
            self,
            b"en-US\0".as_ptr(),
            PackageList,
            StringId,
            data.as_mut_ptr(),
            &mut len,
            0
        )?;
        data.truncate(len);

        let mut string = String::new();
        for &w in data.iter() {
            if w == 0 {
                break;
            }
            let c = unsafe { char::from_u32_unchecked(w as u32) };
            string.push(c);
        }
        Ok(string)
    }
}

impl Protocol<HiiStringProtocol> for &'static mut HiiStringProtocol {
    fn guid() -> Guid {
        HII_STRING_PROTOCOL_GUID
    }

    fn new(inner: &'static mut HiiStringProtocol) -> Self {
        inner
    }
}

// } TODO: Move to uefi library

// TODO: move to uefi library {
#[repr(C)]
pub struct ListEntry<T> {
    Flink: *mut ListEntry<T>,
    Blink: *mut ListEntry<T>,
}

impl<T> ListEntry<T> {
    pub fn previous(&self) -> Option<&Self> {
        if self.Blink.is_null() {
            None
        } else {
            Some(unsafe { &*self.Blink })
        }
    }

    pub fn previous_mut(&mut self) -> Option<&mut Self> {
        if self.Blink.is_null() {
            None
        } else {
            Some(unsafe { &mut *self.Blink })
        }
    }

    pub fn next(&self) -> Option<&Self> {
        if self.Flink.is_null() {
            None
        } else {
            Some(unsafe { &*self.Flink })
        }
    }

    pub fn next_mut(&mut self) -> Option<&mut Self> {
        if self.Flink.is_null() {
            None
        } else {
            Some(unsafe { &mut *self.Flink })
        }
    }

    unsafe fn object_at(&self, offset: usize) -> &T {
        let addr = self as *const Self as usize;
        &*((addr - offset) as *const T)
    }

    unsafe fn object_at_mut(&mut self, offset: usize) -> &mut T {
        let addr = self as *mut Self as usize;
        &mut *((addr - offset) as *mut T)
    }
}

pub trait ListEntryObject<T> {
    unsafe fn object(&self) -> &T;

    unsafe fn object_mut(&mut self) -> &mut T;
}

macro_rules! list_entry {
    ($t:ty, $l:tt) => (
        impl ListEntryObject<$t> for ListEntry<$t> {
            unsafe fn object(&self) -> &$t {
                self.object_at(offset_of!($t, $l))
            }

            unsafe fn object_mut(&mut self) -> &mut $t {
                self.object_at_mut(offset_of!($t, $l))
            }
        }
    );
}

pub struct ListEntryIter<'a, T> {
    start: Option<&'a ListEntry<T>>,
    current: Option<&'a ListEntry<T>>,
}

impl<'a, T> Iterator for ListEntryIter<'a, T> where ListEntry<T>: ListEntryObject<T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.take()?;
        let next = current.next();
        if next.map(|x| x as *const _) == self.start.map(|x| x as *const _) {
            self.current = None;
            return None;
        } else {
            self.current = next;
        }
        Some(unsafe { current.object() })
    }
}

#[repr(transparent)]
pub struct ListHead<T>(ListEntry<T>);

impl<T> ListHead<T> {
    pub fn iter(&self) -> ListEntryIter<T> {
        let next = self.0.next();
        ListEntryIter {
            start: next,
            current: next,
        }
    }
}
// } TODO: move to uefi library

#[repr(C)]
pub struct QuestionOption {
    pub Signature: usize,
    pub Link: ListEntry<QuestionOption>,
    pub OptionOpCodePtr: *const IfrOneOfOption,
    pub ImageId: ImageId,
    pub AnimationId: AnimationId,
}
list_entry!(QuestionOption, Link);

impl QuestionOption {
    pub fn OptionOpCode(&self) -> Option<&IfrOneOfOption> {
        if self.OptionOpCodePtr.is_null() {
            None
        } else {
            Some(unsafe { &*self.OptionOpCodePtr })
        }
    }
}

#[repr(C)]
pub struct StatementErrorInfo {
    pub StringId: StringId,
    pub TimeOut: u8,
}

pub type ValidateQuestion = extern "win64" fn (
    Form: &Form,
    Statement: &Statement,
    Value: &HiiValue,
    ErrorInfo: &mut StatementErrorInfo,
) -> u32;

pub type PasswordCheck = extern "win64" fn(
    Form: &Form,
    Statement: &Statement,
    PasswordString: *const u16
) -> Status;

#[repr(C)]
pub struct Statement {
    pub Signature: usize,
    pub Version: usize,
    pub DisplayLink: ListEntry<Statement>,
    pub OpCodePtr: *const IfrOpHeader,
    pub CurrentValue: HiiValue,
    pub SettingChangedFlag: bool,
    pub NestStatementList: ListHead<Statement>,
    pub OptionListHead: ListHead<QuestionOption>,
    pub Attribute: u32,
    pub ValidateQuestion: Option<ValidateQuestion>,
    pub PasswordCheck: Option<PasswordCheck>,
    pub ImageId: ImageId,
    pub AnimationId: AnimationId,
}
list_entry!(Statement, DisplayLink);

impl Statement {
    pub fn OpCode(&self) -> Option<&IfrOpHeader> {
        if self.OpCodePtr.is_null() {
            None
        } else {
            Some(unsafe { &*self.OpCodePtr })
        }
    }
}

#[repr(C)]
pub struct ScreenDescriptor {
    pub LeftColumn: usize,
    pub RightColumn: usize,
    pub TopRow: usize,
    pub BottomRow: usize,
}

#[repr(C)]
pub struct HotKey {
    pub Signature: usize,
    pub Link: ListEntry<HotKey>,
    pub KeyData: *const TextInputKey,
    pub Action: u32,
    pub DefaultId: u16,
    pub HelpString: *const u16,
}
list_entry!(HotKey, Link);

#[repr(C)]
pub struct Form {
    pub Signature: usize,
    pub Version: usize,
    pub StatementListHead: ListHead<Statement>,
    pub StatementListOSF: ListHead<Statement>,
    pub ScreenDimensions: *const ScreenDescriptor,
    pub FormSetGuid: Guid,
    pub HiiHandle: HiiHandle,
    pub FormId: u16,
    pub FormTitle: StringId,
    pub Attribute: u32,
    pub SettingChangedFlag: bool,
    pub HighlightedStatement: *const Statement,
    pub FormRefreshEvent: Event,
    pub HotKeyListHead: ListHead<HotKey>,
    pub ImageId: ImageId,
    pub AnimationId: AnimationId,
    pub BrowserStatus: u32,
    pub ErrorString: *const u16,
}

#[repr(C)]
pub struct UserInput {
    pub SelectedStatement: *const Statement,
    pub InputValue: HiiValue,
    pub Action: u32,
    pub DefaultId: u16,
}

#[repr(C)]
#[allow(non_snake_case)]
pub struct Fde {
    pub FormDisplay: extern "win64" fn(FormData: &Form, UserInputData: &mut UserInput) -> Status,
    pub ExitDisplay: extern "win64" fn(),
    pub ConfirmDataChange: extern "win64" fn() -> usize,
}


static FONT_TTF: &'static [u8] = include_bytes!("../res/FiraSans-Regular.ttf");
static CHECKBOX_CHECKED_BMP: &'static [u8] = include_bytes!("../res/checkbox_checked.bmp");
static CHECKBOX_UNCHECKED_BMP: &'static [u8] = include_bytes!("../res/checkbox_unchecked.bmp");

static mut DISPLAY: *mut Display = ptr::null_mut();
static mut FONT: *const Font = ptr::null_mut();
static mut CHECKBOX_CHECKED: *const Image = ptr::null_mut();
static mut CHECKBOX_UNCHECKED: *const Image = ptr::null_mut();

struct ElementOption {
    option_ptr: *const QuestionOption,
    prompt: String,
    value: IfrTypeValueEnum,
}

struct Element {
    statement_ptr: *const Statement,
    prompt: String,
    help: String,
    value: IfrTypeValueEnum,
    options: Vec<ElementOption>,
    selectable: bool,
    editable: bool,
    list: bool,
    list_i: usize,
    buffer_opt: Option<&'static mut [u8]>,
}

fn form_display_inner(form: &Form, user_input: &mut UserInput) -> Result<()> {
    debugln!();
    debugln!("form_display");
    debugln!("FORM_DISPLAY_ENGINE_FORM {}, {:?}", mem::size_of::<Form>(), span_of!(Form, HotKeyListHead));
    debugln!("BROWSER_HOT_KEY {}, {:?}", mem::size_of::<HotKey>(), span_of!(HotKey, Link));

    let hii_string = <&'static mut HiiStringProtocol>::one()?;

    let string = |string_id: StringId| -> Result<String> {
         hii_string.string(form.HiiHandle, string_id)
    };

    for hotkey in form.HotKeyListHead.iter() {
        let hotkey_ptr = hotkey as *const _;
        debugln!("  hotkey: {:p}, {:x?}", hotkey_ptr, unsafe {
            slice::from_raw_parts(
                hotkey_ptr as *const u8,
                mem::size_of_val(hotkey)
            )
        });
        debugln!("    key: {:p}, {:?}", hotkey.KeyData, unsafe { *hotkey.KeyData });
        debugln!("    action: {:#x}", hotkey.Action);
        debugln!("    defaultid: {:#x}", hotkey.DefaultId);
        debugln!("    help: {:p}, {}", hotkey.HelpString, ffi::nstr(hotkey.HelpString));
    }

    let mut selected = !0;
    let mut editing = false;
    let mut elements = Vec::new();
    for statement in form.StatementListHead.iter() {
        let statement_ptr = statement as *const _;
        debugln!("statement: {:p}", statement_ptr);

        let mut options = Vec::new();
        for option in statement.OptionListHead.iter() {
            let option_ptr = option as *const _;
            debugln!("  option: {:p}", option_ptr);
            if let Some(op) = option.OptionOpCode() {
                let value = unsafe {
                    op.Value.to_enum(op.Kind)
                };
                debugln!("    {:?}: {:?}", op.Option, value);
                options.push(ElementOption {
                    option_ptr,
                    prompt: string(op.Option).unwrap_or(String::new()),
                    value,
                });
            }
        }

        let add_element = |header: IfrStatementHeader, selectable: bool, editable: bool, list: bool| {
            let value = unsafe {
                statement.CurrentValue.Value.to_enum(statement.CurrentValue.Kind)
            };
            debugln!("    {:?}: {:?}", header, value);
            let buffer_opt = if statement.CurrentValue.Buffer.is_null() {
                None
            } else {
                let buffer = unsafe {
                    slice::from_raw_parts_mut(
                        statement.CurrentValue.Buffer,
                        statement.CurrentValue.BufferLen as usize
                    )
                };
                debugln!("      buffer: {:?}", buffer);
                // Order list according to buffer
                if list {
                    let mut offset = 0;
                    for i in 0..options.len() {
                        for j in i..options.len() {
                            macro_rules! check_option {
                                ($x:ident) => ({
                                    let next_offset = offset + mem::size_of_val(&$x);
                                    if next_offset <= buffer.len() {
                                        let mut x_copy = $x;
                                        unsafe {
                                            ptr::copy(
                                                buffer.as_ptr().add(offset) as *const _,
                                                &mut x_copy,
                                                1
                                            );
                                        };
                                        if $x == x_copy {
                                            offset = next_offset;
                                            true
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    }
                                });
                            }
                            let matches = match options[j].value {
                                IfrTypeValueEnum::U8(u8) => check_option!(u8),
                                IfrTypeValueEnum::U16(u16) => check_option!(u16),
                                IfrTypeValueEnum::U32(u32) => check_option!(u32),
                                IfrTypeValueEnum::U64(u64) => check_option!(u64),
                                other => {
                                    debugln!("unsupported option in list: {:?}", other);
                                    false
                                },
                            };
                            if matches {
                                if i != j {
                                    options.swap(i, j);
                                }
                                break;
                            }
                        }
                    }
                }
                Some(buffer)
            };
            if statement_ptr == form.HighlightedStatement || (selected == !0 && selectable) {
                selected = elements.len();
            }
            elements.push(Element {
                statement_ptr,
                prompt: string(header.Prompt).unwrap_or(String::new()),
                help: string(header.Help).unwrap_or(String::new()),
                value,
                options,
                selectable,
                editable,
                list,
                list_i: 0,
                buffer_opt,
            });
        };

        if let Some(op) = statement.OpCode() {
            debugln!("  {:?}", op);
            macro_rules! cast {
                ($type:ty) => ({
                    debugln!("    casting {} to {}", op.Length(), mem::size_of::<$type>());
                    op.cast::<$type>()
                });
            }
            match op.OpCode {
                IfrOpCode::Action => if let Some(action) = unsafe { cast!(IfrAction) } {
                    add_element(action.QuestionHeader.Header, true, false, false);
                },
                IfrOpCode::Checkbox => if let Some(checkbox) = unsafe { cast!(IfrCheckbox) } {
                    add_element(checkbox.Question.Header, true, true, false);
                },
                IfrOpCode::Numeric => if let Some(numeric) = unsafe { cast!(IfrNumeric) } {
                    add_element(numeric.Question.Header, true, true, false);
                },
                IfrOpCode::OneOf => if let Some(one_of) = unsafe { cast!(IfrOneOf) } {
                    add_element(one_of.Question.Header, true, true, false);
                },
                IfrOpCode::OrderedList => if let Some(ordered_list) = unsafe { cast!(IfrOrderedList) } {
                    add_element(ordered_list.Question.Header, true, true, true);
                },
                IfrOpCode::Ref => if let Some(ref_) = unsafe { cast!(IfrRef) } {
                    add_element(ref_.Question.Header, true, false, false);
                },
                IfrOpCode::Subtitle => if let Some(subtitle) = unsafe { cast!(IfrSubtitle) } {
                    add_element(subtitle.Statement, false, false, false);
                },
                _ => ()
            }
        }
    }

    let mut display = unsafe {
        if DISPLAY.is_null() {
            let display = Display::new(Output::one()?);
            DISPLAY = Box::into_raw(Box::new(display));
        }
        ScaledDisplay::new(&mut *DISPLAY)
    };

    let font = unsafe {
        if FONT.is_null() {
            let font = match Font::from_data(FONT_TTF) {
                Ok(ok) => ok,
                Err(err) => {
                    println!("failed to parse font: {}", err);
                    return Err(Error::NotFound);
                }
            };
            FONT = Box::into_raw(Box::new(font));
        }
        &*FONT
    };

    let checkbox_checked = unsafe {
        if CHECKBOX_CHECKED.is_null() {
            let image = match image::bmp::parse(CHECKBOX_CHECKED_BMP) {
                Ok(ok) => ok,
                Err(err) => {
                    println!("failed to parse checkbox checked: {}", err);
                    return Err(Error::NotFound);
                }
            };
            CHECKBOX_CHECKED = Box::into_raw(Box::new(image));
        }
        &*CHECKBOX_CHECKED
    };

    let checkbox_unchecked = unsafe {
        if CHECKBOX_UNCHECKED.is_null() {
            let image = match image::bmp::parse(CHECKBOX_UNCHECKED_BMP) {
                Ok(ok) => ok,
                Err(err) => {
                    println!("failed to parse checkbox unchecked: {}", err);
                    return Err(Error::NotFound);
                }
            };
            CHECKBOX_UNCHECKED = Box::into_raw(Box::new(image));
        }
        &*CHECKBOX_UNCHECKED
    };

    let title_opt = string(form.FormTitle).ok();
    'display: loop {
        let (display_w, display_h) = (display.width(), display.height());

        // Style {
        let background_color = Color::rgb(0x33, 0x30, 0x2F);
        let highlight_color = Color::rgb(0xde, 0x88, 0x00);
        let outline_color = Color::rgba(0xfe, 0xff, 0xff, 0xc4);
        let text_color = Color::rgb(0xed, 0xed, 0xed);
        let padding_lr = 8;
        let padding_tb = 4;
        let margin_lr = 16;
        let margin_tb = 8;
        let rect_radius = 4;
        // } Style

        display.set(background_color);

        let title_font_size = 40.0;
        let font_size = 32.0; // (display_h as f32) / 26.0

        let draw_pretty_box = |display: &mut ScaledDisplay, x: i32, y: i32, w: u32, h: u32, highlighted: bool| {
            let checkbox = if highlighted {
                // Center
                display.rect(
                    x - padding_lr,
                    y - padding_tb + rect_radius,
                    w + padding_lr as u32 * 2,
                    h + (padding_tb - rect_radius) as u32 * 2,
                    highlight_color
                );

                // Top middle
                display.rect(
                    x - padding_lr + rect_radius,
                    y - padding_tb,
                    w + (padding_lr - rect_radius) as u32 * 2,
                    rect_radius as u32,
                    highlight_color,
                );

                // Bottom middle
                display.rect(
                    x - padding_lr + rect_radius,
                    y + h as i32 + padding_tb - rect_radius,
                    w + (padding_lr - rect_radius) as u32 * 2,
                    rect_radius as u32,
                    highlight_color,
                );

                checkbox_checked
            } else {
                // Top middle
                display.rect(
                    x - padding_lr + rect_radius,
                    y - padding_tb,
                    w + (padding_lr - rect_radius) as u32 * 2,
                    2,
                    outline_color
                );

                // Bottom middle
                display.rect(
                    x - padding_lr + rect_radius,
                    y + h as i32 + padding_tb - 2,
                    w + (padding_lr - rect_radius) as u32 * 2,
                    2,
                    outline_color
                );

                // Left middle
                display.rect(
                    x - padding_lr,
                    y - padding_tb + rect_radius,
                    2,
                    h + (padding_tb - rect_radius) as u32 * 2,
                    outline_color
                );

                // Right middle
                display.rect(
                    x + w as i32 + padding_lr - 2,
                    y - padding_tb + rect_radius,
                    2,
                    h + (padding_tb - rect_radius) as u32 * 2,
                    outline_color
                );

                checkbox_unchecked
            };

            // Top left
            checkbox.roi(
                0,
                0,
                rect_radius as u32,
                rect_radius as u32
            ).draw(
                display,
                x - padding_lr,
                y - padding_tb
            );

            // Top right
            checkbox.roi(
                checkbox.width() - rect_radius as u32,
                0,
                rect_radius as u32,
                rect_radius as u32
            ).draw(
                display,
                x + w as i32 + padding_lr - rect_radius,
                y - padding_tb
            );

            // Bottom left
            checkbox.roi(
                0,
                checkbox.height() - rect_radius as u32,
                rect_radius as u32,
                rect_radius as u32
            ).draw(
                display,
                x - padding_lr,
                y + h as i32 + padding_tb - rect_radius
            );

            // Bottom right
            checkbox.roi(
                checkbox.width() - rect_radius as u32,
                checkbox.height() - rect_radius as u32,
                rect_radius as u32,
                rect_radius as u32
            ).draw(
                display,
                x + w as i32 + padding_lr - rect_radius,
                y + h as i32 + padding_tb - rect_radius
            );
        };

        let draw_text_box = |display: &mut ScaledDisplay, x: i32, y: i32, rendered: &Text, pretty_box: bool, highlighted: bool| {
            if pretty_box {
                draw_pretty_box(display, x, y, rendered.width(), rendered.height(), highlighted);
            }
            rendered.draw(display, x, y, text_color);
        };

        let draw_check_box = |display: &mut ScaledDisplay, x: i32, y: i32, value: bool| -> i32 {
            let checkbox = if value {
                checkbox_checked
            } else {
                checkbox_unchecked
            };
            checkbox.draw(display, x, y);
            checkbox.height() as i32
        };

        let draw_value_box = |display: &mut ScaledDisplay, x: i32, y: i32, value: &IfrTypeValueEnum, highlighted: bool| -> i32 {
            //TODO: Do not format in drawing loop
            let value_string = match value {
                IfrTypeValueEnum::U8(value) => format!("{}", value),
                IfrTypeValueEnum::U16(value) => format!("{}", value),
                IfrTypeValueEnum::U32(value) => format!("{}", value),
                IfrTypeValueEnum::U64(value) => format!("{}", value),
                IfrTypeValueEnum::Bool(value) => return draw_check_box(display, x, y, *value),
                other => format!("{:?}", other),
            };

            // TODO: Do not render in drawing loop
            let rendered = font.render(&value_string, font_size);
            draw_text_box(display, x, y, &rendered, true, highlighted);
            rendered.height() as i32
        };

        let mut y = margin_tb;

        if editing && elements.get(selected).map(|e| ! e.options.is_empty()).unwrap_or(false) {
            if let Some(element) = elements.get(selected) {
                {
                    // TODO: Do not render in drawing loop
                    let rendered = font.render(&element.prompt, title_font_size);
                    let x = (display_w as i32 - rendered.width() as i32) / 2;
                    draw_text_box(&mut display, x, y, &rendered, false, false);
                    y += rendered.height() as i32 + margin_tb;
                }

                display.rect(
                    0,
                    y,
                    display_w,
                    1,
                    Color::rgb(0xac, 0xac, 0xac)
                );
                y += margin_tb * 2;

                if element.options.is_empty() {
                    let h = draw_value_box(&mut display, margin_lr, y, &element.value, true);
                    y += h + margin_tb;
                } else if element.list {
                    for (i, option) in element.options.iter().enumerate() {
                        // TODO: Do not render in drawing loop
                        let rendered = font.render(&option.prompt, font_size);
                        let highlighted = i == element.list_i;
                        draw_text_box(&mut display, margin_lr, y, &rendered, highlighted, highlighted);
                        y += rendered.height() as i32 + margin_tb;
                    }
                } else {
                    for option in element.options.iter() {
                        // TODO: Do not render in drawing loop
                        let rendered = font.render(&option.prompt, font_size);
                        let highlighted = option.value == element.value;
                        draw_text_box(&mut display, margin_lr, y, &rendered, highlighted, highlighted);
                        y += rendered.height() as i32 + margin_tb;
                    }
                }
            } else {
                editing = false;
                continue 'display;
            }
        } else {
            if let Some(ref title) = title_opt {
                // TODO: Do not render in drawing loop
                let rendered = font.render(&title, title_font_size);
                let x = (display_w as i32 - rendered.width() as i32) / 2;
                draw_text_box(&mut display, x, y, &rendered, false, false);
                y += rendered.height() as i32 + margin_tb;
            }

            display.rect(
                0,
                y,
                display_w,
                1,
                Color::rgb(0xac, 0xac, 0xac)
            );
            y += margin_tb * 2;

            for (i, element) in elements.iter().enumerate() {
                let highlighted = i == selected;
                let h = {
                    // TODO: Do not render in drawing loop
                    let rendered = font.render(&element.prompt, font_size);
                    draw_text_box(&mut display, margin_lr, y, &rendered, highlighted, highlighted && ! editing);
                    rendered.height() as i32
                };

                let x = display_w as i32 / 2;
                if element.list {
                    let start_y = y;
                    let mut w = 0;
                    for option in element.options.iter() {
                        let rendered = font.render(&option.prompt, font_size);
                        draw_text_box(&mut display, x, y, &rendered, false, false);
                        w = cmp::max(w, rendered.width());
                        y += rendered.height() as i32 + margin_tb;
                    }
                    if y > start_y {
                        draw_pretty_box(&mut display, x, start_y, w, (y - start_y - margin_tb) as u32, highlighted && editing);
                    }
                    y -= h + margin_tb;
                } else if let Some(option) = element.options.iter().find(|o| o.value == element.value) {
                    // TODO: Do not render in drawing loop
                    let rendered = font.render(&option.prompt, font_size);
                    draw_text_box(&mut display, x, y, &rendered, true, highlighted && editing);
                } else if element.editable {
                    draw_value_box(&mut display, x, y, &element.value, highlighted && editing);
                }

                y += h + margin_tb;
            }
        }

        // Draw footer
        {
            y = display_h as i32;

            if let Some(element) = elements.get(selected) {
                let rendered = font.render(&element.help, font_size);
                let x = (display_w as i32 - rendered.width() as i32) / 2;
                y -= rendered.height() as i32 + margin_tb;
                draw_text_box(&mut display, x, y, &rendered, false, false);
            } else {
                let rendered = font.render("https://github.com/system76/firmware-setup", font_size);
                let x = (display_w as i32 - rendered.width() as i32) / 2;
                y -= rendered.height() as i32 + margin_tb;
                draw_text_box(&mut display, x, y, &rendered, false, false);
            }

            let mut x = margin_lr;
            for (i, hotkey) in form.HotKeyListHead.iter().enumerate() {
                let rendered = font.render(&ffi::nstr(hotkey.HelpString), font_size);
                if i == 0 {
                    y -= rendered.height() as i32 + margin_tb;
                }
                draw_text_box(&mut display, x, y, &rendered, true, false);
                x += rendered.width() as i32 + margin_lr * 2;
            }

            y -= margin_tb * 3 / 2;
            display.rect(
                0,
                y,
                display_w,
                1,
                Color::rgb(0xac, 0xac, 0xac)
            );
        }


        display.sync();

        let raw_key = raw_key()?;
        for hotkey in form.HotKeyListHead.iter() {
            let key_data = unsafe { &*hotkey.KeyData };
            if key_data.ScanCode == raw_key.ScanCode && key_data.UnicodeChar == raw_key.UnicodeChar {
                debugln!("pressed {}", ffi::nstr(hotkey.HelpString));
                user_input.Action = hotkey.Action;
                user_input.DefaultId = hotkey.DefaultId;
                break 'display;
            }
        }

        let key = Key::from(raw_key);
        debugln!("{:?}", key);
        match key {
            Key::Enter => {
                if let Some(mut element) = elements.get_mut(selected) {
                    if element.editable && ! editing {
                        editing = true;
                    } else {
                        user_input.SelectedStatement = element.statement_ptr;
                        unsafe {
                            ptr::copy(
                                &(*element.statement_ptr).CurrentValue,
                                &mut user_input.InputValue,
                                1
                            );
                        }
                        if editing {
                            if element.list {
                                let mut offset = 0;
                                if let Some(ref mut buffer) = element.buffer_opt {
                                    for option in element.options.iter() {
                                        macro_rules! copy_option {
                                            ($x:ident) => ({
                                                let next_offset = offset + mem::size_of_val(&$x);
                                                if next_offset <= buffer.len() {
                                                    unsafe {
                                                        ptr::copy(
                                                            &$x,
                                                            buffer.as_mut_ptr().add(offset) as *mut _,
                                                            1
                                                        )
                                                    }
                                                }
                                                offset = next_offset;
                                            });
                                        }
                                        match option.value {
                                            IfrTypeValueEnum::U8(u8) => copy_option!(u8),
                                            IfrTypeValueEnum::U16(u16) => copy_option!(u16),
                                            IfrTypeValueEnum::U32(u32) => copy_option!(u32),
                                            IfrTypeValueEnum::U64(u64) => copy_option!(u64),
                                            other => {
                                                debugln!("unsupported option in list: {:?}", other);
                                            },
                                        }
                                    }
                                    if offset < buffer.len() {
                                        for i in offset..buffer.len() {
                                            buffer[i] = 0;
                                        }
                                    }
                                    debugln!("modified: {:?}", buffer);
                                } else {
                                    debugln!("list without buffer");
                                }
                            } else {
                                let (kind, value) = unsafe { element.value.to_union() };
                                user_input.InputValue.Kind = kind;
                                user_input.InputValue.Value = value;
                            }
                            editing = false;
                        }
                        break 'display;
                    }
                }
            },
            Key::Escape => {
                if editing {
                    editing = false;
                } else {
                    user_input.Action = 1 << 17;
                    break 'display;
                }
            },
            Key::Down => {
                if editing {
                    if let Some(mut element) = elements.get_mut(selected) {
                        if element.list {
                            if element.list_i + 1 < element.options.len() {
                                element.list_i += 1;
                            } else {
                                element.list_i = 0;
                            }
                        } else {
                            let i_opt = element.options.iter().position(|o| o.value == element.value);
                            if let Some(mut i) = i_opt {
                                if i + 1 < element.options.len() {
                                    i += 1;
                                } else {
                                    i = 0;
                                }
                                element.value = element.options[i].value;
                            }
                        }
                    }
                } else if selected != !0 {
                    let start = selected;
                    loop {
                        if selected + 1 < elements.len() {
                            selected += 1;
                        } else {
                            selected = 0;
                        }
                        if let Some(element) = elements.get(selected) {
                            if element.selectable {
                                break;
                            }
                        }
                        if selected == start {
                            break;
                        }
                    }
                }
            },
            Key::Up => {
                if editing {
                    if let Some(mut element) = elements.get_mut(selected) {
                        if element.list {
                            if element.list_i > 0 {
                                element.list_i -= 1;
                            } else if ! element.options.is_empty() {
                                element.list_i = element.options.len() - 1;
                            }
                        } else {
                            let i_opt = element.options.iter().position(|o| o.value == element.value);
                            if let Some(mut i) = i_opt {
                                if i > 0 {
                                    i -= 1;
                                } else {
                                    i = element.options.len() - 1;
                                }
                                element.value = element.options[i].value;
                            }
                        }
                    }
                } else if selected != !0 {
                    let start = selected;
                    loop {
                        if selected > 0 {
                            selected -= 1;
                        } else {
                            selected = cmp::max(elements.len(), 1) - 1;
                        }
                        if let Some(element) = elements.get(selected) {
                            if element.selectable {
                                break;
                            }
                        }
                        if selected == start {
                            break;
                        }
                    }
                }
            },
            Key::PageDown => {
                if editing {
                    if let Some(mut element) = elements.get_mut(selected) {
                        if element.list {
                            if element.list_i + 1 < element.options.len() {
                                element.options.swap(element.list_i, element.list_i + 1);
                                element.list_i += 1;
                            }
                        }
                    }
                }
            },
            Key::PageUp => {
                if editing {
                    if let Some(mut element) = elements.get_mut(selected) {
                        if element.list {
                            if element.list_i > 0 {
                                element.list_i -= 1;
                                element.options.swap(element.list_i, element.list_i + 1);
                            }
                        }
                    }
                }
            },
            _ => (),
        }
    }

    debugln!("selected: {:p}, action: {:#x}", user_input.SelectedStatement, user_input.Action);

    Ok(())
}

extern "win64" fn form_display(form: &Form, user_input: &mut UserInput) -> Status {
    match form_display_inner(form, user_input) {
        Ok(()) => Status::from_ok(0),
        Err(err) => Status::from_error(err),
    }
}

extern "win64" fn exit_display() {
    debugln!("exit_display");
}

extern "win64" fn confirm_data_change() -> usize {
    debugln!("confirm_data_change");
    0
}

impl Fde {
    pub fn install() -> Result<()> {
        let guid = Guid(0x9bbe29e9, 0xfda1, 0x41ec, [0xad, 0x52, 0x45, 0x22, 0x13, 0x74, 0x2d, 0x2e]);

        let uefi = unsafe { std::system_table_mut() };

        let current = unsafe {
            let mut interface = 0;
            (uefi.BootServices.LocateProtocol)(&guid, 0, &mut interface)?;
            &mut *(interface as *mut Fde)
        };

        debugln!("Current FDE: {:#p}", current);

        current.FormDisplay = form_display;
        current.ExitDisplay = exit_display;
        current.ConfirmDataChange = confirm_data_change;

        Ok(())
    }
}