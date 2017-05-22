/*
 * Copyright (c) 2017 Boucher, Antoni <bouanto@zoho.com>
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy of
 * this software and associated documentation files (the "Software"), to deal in
 * the Software without restriction, including without limitation the rights to
 * use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
 * the Software, and to permit persons to whom the Software is furnished to do so,
 * subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
 * FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
 * COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
 * IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
 * CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::mem;
use std::sync::Mutex;

use quote::{Tokens, ToTokens};
use syn::{self, Expr, Path, Ty, parse_expr, parse_item, parse_path};
use syn::Delimited;
use syn::DelimToken::{Brace, Bracket, Paren};
use syn::ItemKind::Mac;
use syn::Lit::Str;
use syn::StrStyle::Cooked;
use syn::TokenTree::{self, Token};
use syn::Token::{At, BinOp, Colon, Comma, Dot, Eq, FatArrow, Gt, Ident, Literal, Lt, ModSep, Pound};
use syn::BinOpToken::Or;

use self::DefaultParam::*;
use self::EventValue::*;
use self::EventValueReturn::*;
use self::EitherWidget::*;
use self::IsEventOrNot::*;

pub const RELM_WIDGET_CLONE_IDENT: &str = "__relm_widget_self_clone";
pub const RELM_WIDGET_SELF_IDENT: &str = "__relm_widget_self";

lazy_static! {
    static ref NAMES_INDEX: Mutex<HashMap<String, u32>> = Mutex::new(HashMap::new());
}

type ChildEvents = HashMap<(String, String), Event>;

#[derive(Clone, Copy, PartialEq)]
enum DefaultParam {
    DefaultNoParam,
    DefaultOneParam,
}

#[derive(Debug)]
pub enum EventValueReturn {
    CallReturn(Tokens),
    Return(Tokens, Tokens),
    WithoutReturn(Tokens),
}

#[derive(Debug)]
pub enum EventValue {
    CurrentWidget(EventValueReturn),
    ForeignWidget(Tokens, EventValueReturn),
}

#[derive(Debug)]
pub struct Event {
    pub async: bool,
    pub params: Vec<syn::Ident>,
    pub use_self: bool,
    pub value: EventValue,
}

impl Event {
    fn new() -> Self {
        Event {
            async: false,
            params: vec![syn::Ident::new("_")],
            use_self: false,
            value: CurrentWidget(WithoutReturn(Tokens::new())),
        }
    }
}

#[derive(PartialEq)]
enum IsEventOrNot {
    IsEvent,
    NotEvent,
}

pub struct Widget {
    pub child_events: ChildEvents,
    pub child_properties: HashMap<String, Expr>,
    pub children: Vec<Widget>,
    pub container_type: Option<Option<String>>,
    pub init_parameters: Vec<Expr>,
    pub name: syn::Ident,
    pub parent_id: Option<String>,
    pub properties: HashMap<String, Expr>,
    pub typ: Path,
    pub widget: EitherWidget,
}

impl Widget {
    fn new_gtk(widget: GtkWidget, typ: Path, init_parameters: Vec<Expr>, children: Vec<Widget>,
        properties: HashMap<String, Expr>, child_properties: HashMap<String, Expr>, child_events: ChildEvents) -> Self
    {
        let name = gen_widget_name(&typ);
        Widget {
            child_events,
            child_properties,
            children,
            container_type: None,
            init_parameters,
            name: syn::Ident::new(name),
            parent_id: None,
            properties,
            typ,
            widget: Gtk(widget),
        }
    }

    fn new_relm(widget: RelmWidget, typ: Path, init_parameters: Vec<Expr>, children: Vec<Widget>,
        properties: HashMap<String, Expr>, child_properties: HashMap<String, Expr>, child_events: ChildEvents) -> Self
    {
        let mut name = gen_widget_name(&typ);
        // Relm widgets are not used in the update() method; they are only saved to avoid dropping
        // their channel too soon.
        // So prepend an underscore to hide a warning.
        name.insert(0, '_');
        Widget {
            child_events,
            child_properties,
            children,
            container_type: None,
            init_parameters,
            name: syn::Ident::new(name),
            parent_id: None,
            properties,
            typ,
            widget: Relm(widget),
        }
    }
}

#[derive(Debug)]
pub enum EitherWidget {
    Gtk(GtkWidget),
    Relm(RelmWidget),
}

#[derive(Debug)]
pub struct GtkWidget {
    pub construct_properties: HashMap<syn::Ident, Expr>,
    pub events: HashMap<String, Event>,
    pub relm_name: Option<Ty>,
    pub save: bool,
}

impl GtkWidget {
    fn new() -> Self {
        GtkWidget {
            construct_properties: HashMap::new(),
            events: HashMap::new(),
            relm_name: None,
            save: false,
        }
    }
}

#[derive(Debug)]
pub struct RelmWidget {
    pub events: HashMap<String, Vec<Event>>,
    pub gtk_events: HashMap<String, Event>,
}

impl RelmWidget {
    fn new() -> Self {
        RelmWidget {
            events: HashMap::new(),
            gtk_events: HashMap::new(),
        }
    }
}

fn last_segment_lowercase(path: &Path) -> bool {
    let last_segment = path.segments.last().expect("parsed name should have at least one segment");
    if last_segment.ident.as_ref().chars().next()
        .expect("last_segment should have at least one character").is_lowercase()
    {
        true
    }
    else {
        false
    }
}

pub fn parse(tokens: &[TokenTree]) -> Widget {
    let tokens =
        if let Token(Literal(Str(ref relm_view_file, _))) = tokens[0] {
            // TODO: also support glade file.
            let mut file = File::open(relm_view_file).expect("File::open() in parse()");
            let mut file_content = String::new();
            file.read_to_string(&mut file_content).expect("read_to_string() in parse()");
            let item = parse_item(&file_content).expect("parse_item() in parse()");
            if let Mac(syn::Mac { tts, .. }) = item.node {
                if let TokenTree::Delimited(Delimited { ref tts, .. }) = tts[0] {
                    tts.clone()
                }
                else {
                    panic!("Expected delimited macro")
                }
            }
            else {
                panic!("Expecting a macro")
            }
        }
        else {
            tokens.to_vec()
        };
    let (mut widget, _, parent_id) = parse_child(&tokens, true);
    widget.parent_id = parent_id;
    widget
}

fn parse_widget(tokens: &[TokenTree], save: bool) -> (Widget, &[TokenTree]) {
    let (gtk_type, mut tokens) = parse_qualified_name(tokens);
    let mut gtk_widget = GtkWidget::new();
    let mut init_parameters = vec![];
    let mut children = vec![];
    let mut properties = HashMap::new();
    let mut child_properties = HashMap::new();
    let mut child_events = HashMap::new();
    gtk_widget.save = save;
    if let TokenTree::Delimited(Delimited { delim: Paren, ref tts }) = tokens[0] {
        if let TokenTree::Delimited(Delimited { delim: Brace, ref tts }) = tts[0] {
            gtk_widget.construct_properties = parse_hash(tts);
        }
        else {
            init_parameters = parse_comma_list(tts);
        }
        tokens = &tokens[1..];
    }
    if let TokenTree::Delimited(Delimited { delim: Brace, ref tts }) = tokens[0] {
        let mut tts = &tts[..];
        while !tts.is_empty() {
            let (async, new_tts) = try_parse_async(tts);
            tts = new_tts;
            if !async && (tts[0] == Token(Pound) || try_parse_name(tts).is_some()) {
                let (child, new_tts, _) = parse_child(tts, false);
                tts = new_tts;
                children.push(child);
            }
            else {
                // Property or event.
                let (ident, _) = parse_ident(tts);
                tts = &tts[1..];
                match tts[0] {
                    Token(Colon) => {
                        tts = parse_value_or_child_properties(tts, ident, &mut child_properties, &mut properties);
                    },
                    Token(Dot) => {
                        let child_name = ident;
                        let (ident, new_tts) = parse_ident(&tts[1..]);
                        let (event, new_tts) = parse_event(new_tts, DefaultOneParam);
                        child_events.insert((child_name, ident), event);
                        tts = new_tts;
                    },
                    TokenTree::Delimited(Delimited { delim: Paren, .. }) | Token(FatArrow) => {
                        let (mut event, new_tts) = parse_event(tts, DefaultOneParam);
                        event.async = async;
                        gtk_widget.events.insert(ident, event);
                        tts = new_tts;
                    },
                    _ => panic!("Expected `:` or `(` but found `{:?}` in view! macro", tts[0]),
                }
            }

            if tts.first() == Some(&Token(Comma)) {
                tts = &tts[1..];
            }
        }
    }
    else {
        panic!("Expected {{ but found `{:?}` in view! macro", tokens[0]);
    }
    let widget = Widget::new_gtk(gtk_widget, gtk_type, init_parameters, children, properties, child_properties,
                                 child_events);
    (widget, &tokens[1..])
}

fn parse_child(mut tokens: &[TokenTree], root: bool) -> (Widget, &[TokenTree], Option<String>) {
    let (mut attributes, new_tokens) = parse_attributes(tokens);
    let container_type = attributes.remove("container")
        .map(|typ| typ.map(str::to_string));
    tokens = new_tokens;
    let name = attributes.get("name").and_then(|name| *name);
    let (mut widget, new_tokens) =
        if tokens.get(1) == Some(&Token(ModSep)) {
            parse_widget(tokens, name.is_some() || root)
        }
        else {
            parse_relm_widget(tokens)
        };
    if let Some(name) = name {
        widget.name = syn::Ident::new(name);
    }
    widget.container_type = container_type;
    let parent_id = attributes.get("parent").and_then(|opt_str| opt_str.map(str::to_string));
    (widget, new_tokens, parent_id)
}

fn parse_ident(tokens: &[TokenTree]) -> (String, &[TokenTree]) {
    match tokens[0] {
        Token(Ident(ref ident)) => {
            (ident.to_string(), &tokens[1..])
        },
        _ => panic!("Expected ident but found `{:?}` in view! macro", tokens[0]),
    }
}

fn parse_qualified_name(tokens: &[TokenTree]) -> (Path, &[TokenTree]) {
    try_parse_name(tokens)
        .unwrap_or_else(|| panic!("Expected qualified name but found `{:?}` in view! macro", tokens[0]))
}

fn try_parse_name(mut tokens: &[TokenTree]) -> Option<(Path, &[TokenTree])> {
    let mut path_string = String::new();
    let mut angle_level = 0;
    while !tokens.is_empty() {
        match tokens[0] {
            Token(Lt) => angle_level += 1,
            Token(Gt) => angle_level -= 1,
            Token(Comma) if angle_level == 0 => break,
            Token(Comma) | Token(Ident(_)) | Token(ModSep) => (),
            _ => break,
        }
        let mut toks = Tokens::new();
        tokens[0].to_tokens(&mut toks);
        path_string.push_str(&toks.to_string());
        tokens = &tokens[1..];
    }
    match tokens[0] {
        TokenTree::Delimited(_) | Token(Comma) => {
            if let Ok(path) = parse_path(&path_string) {
                if !last_segment_lowercase(&path) {
                    return Some((path, tokens));
                }
            }
        },
        _ => (),
    }
    None
}

fn parse_comma_ident_list(tokens: &[TokenTree]) -> Vec<syn::Ident> {
    let mut params = vec![];
    let mut param = Tokens::new();
    for token in tokens {
        match *token {
            Token(Comma) =>  {
                params.push(syn::Ident::new(param.as_str()));
                param = Tokens::new();
            },
            Token(ref token) => token.to_tokens(&mut param),
            _ => panic!("Expecting Token, but found: `{:?}`", token),
        }
    }
    params.push(syn::Ident::new(param.as_str()));
    params
}

enum HashState {
    InName,
    AfterName,
    InValue,
}

use self::HashState::*;

fn parse_hash(tokens: &[TokenTree]) -> HashMap<syn::Ident, Expr> {
    let mut params = HashMap::new();
    let mut current_param = Tokens::new();
    let mut state = InName;
    let mut name = syn::Ident::new("");
    for token in tokens {
        match state {
            InName => {
                // FIXME: support ident with dash (-).
                if let Token(Ident(ref ident)) = *token {
                    name = syn::Ident::new(ident.as_ref().replace('_', "-"));
                    state = AfterName;
                }
                else {
                    panic!("Expected ident, but found `{:?}` in view! macro", token);
                }
            },
            AfterName => {
                if *token == Token(Colon) {
                    state = InValue;
                }
                else {
                    panic!("Expected colon, but found `{:?}` in view! macro", token);
                }
            },
            InValue => {
                if *token == Token(Comma) {
                    let ident = mem::replace(&mut name, syn::Ident::new(""));
                    params.insert(ident, tokens_to_expr(current_param));
                    current_param = Tokens::new();
                    state = InName;
                }
                else {
                    token.to_tokens(&mut current_param);
                }
            },
        }
    }
    // FIXME: could be an empty hash.
    params.insert(name, tokens_to_expr(current_param));
    params
}

fn parse_comma_list(tokens: &[TokenTree]) -> Vec<Expr> {
    let mut params = vec![];
    let mut current_param = Tokens::new();
    for token in tokens {
        if *token == Token(Comma) {
            params.push(tokens_to_expr(current_param));
            current_param = Tokens::new();
        }
        else {
            token.to_tokens(&mut current_param);
        }
    }
    // FIXME: could be an empty list.
    params.push(tokens_to_expr(current_param));
    params
}

fn parse_event(mut tokens: &[TokenTree], default_param: DefaultParam) -> (Event, &[TokenTree]) {
    let mut event = Event::new();
    if default_param == DefaultNoParam {
        event.params.clear();
    }
    if let TokenTree::Delimited(Delimited { delim: Paren, ref tts }) = tokens[0] {
        event.params = parse_comma_ident_list(tts);
        tokens = &tokens[1..];
    }
    if tokens[0] != Token(FatArrow) {
        panic!("Expected `=>` but found `{:?}` in view! macro", tokens[0]);
    }
    tokens = &tokens[1..];
    event.value =
        // Message sent to another widget.
        if tokens.len() >= 2 && tokens[1] == Token(At) {
            let (event_value, new_tokens, use_self) = parse_event_value(&tokens[2..]);
            event.use_self = use_self;
            let (ident, _) = parse_ident(tokens);
            tokens = new_tokens;
            let mut ident_tokens = Tokens::new();
            ident_tokens.append(ident);
            ForeignWidget(ident_tokens, event_value)
        }
        // Message sent to the same widget.
        else {
            let (event_value, new_tokens, use_self) = parse_event_value(tokens);
            event.use_self = use_self;
            tokens = new_tokens;
            CurrentWidget(event_value)
        };
    (event, tokens)
}

fn parse_event_value(tokens: &[TokenTree]) -> (EventValueReturn, &[TokenTree], bool) {
    if Token(Ident(syn::Ident::new("return"))) == tokens[0] {
        let (value, tokens, use_self) = parse_value(&tokens[1..], IsEvent);
        (CallReturn(value), tokens, use_self)
    }
    else if let TokenTree::Delimited(Delimited { delim: Paren, ref tts }) = tokens[0] {
        let (value1, new_tts, use_self1) = parse_value(tts, IsEvent);
        if new_tts[0] != Token(Comma) {
            panic!("Expected `,` but found `{:?}` in view! macro", new_tts[0]);
        }
        let (value2, _, use_self2) = parse_value(&new_tts[1..], IsEvent);
        (Return(value1, value2), &tokens[1..], use_self1 || use_self2)
    }
    else {
        let (value, tokens, use_self) = parse_value(tokens, IsEvent);
        (WithoutReturn(value), tokens, use_self)
    }
}

fn parse_value_or_child_properties<'a>(tokens: &'a [TokenTree], ident: String,
    child_properties: &mut HashMap<String, Expr>, properties: &mut HashMap<String, Expr>) -> &'a [TokenTree]
{
    match tokens[1] {
        TokenTree::Delimited(Delimited { delim: Brace, tts: ref child_tokens }) => {
            let props = parse_child_properties(child_tokens);
            for (key, value) in props {
                child_properties.insert(key, tokens_to_expr(value));
            }
            &tokens[2..]
        },
        _ => {
            let (value, tts, _) = parse_value(&tokens[1..], NotEvent);
            properties.insert(ident, tokens_to_expr(value));
            tts
        },
    }
}

fn parse_value(tokens: &[TokenTree], is_event: IsEventOrNot) -> (Tokens, &[TokenTree], bool) {
    let mut current_param = Tokens::new();
    let mut i = 0;
    let mut in_closure = false;
    let mut in_closure_value = false;
    let mut use_self = false;
    while i < tokens.len() {
        match tokens[i] {
            Token(Ident(ref ident)) if *ident == syn::Ident::new("self") => {
                use_self = true;
                let new_ident =
                    if is_event == IsEvent || in_closure_value {
                        RELM_WIDGET_CLONE_IDENT
                    }
                    else {
                        "self"
                    };
                Token(Ident(syn::Ident::new(new_ident))).to_tokens(&mut current_param)
            },
            Token(Comma) if !in_closure => break,
            ref token@Token(BinOp(Or)) => {
                if in_closure {
                    in_closure_value = true;
                }
                in_closure = !in_closure;
                token.to_tokens(&mut current_param);
            },
            ref token => token.to_tokens(&mut current_param),
        }
        i += 1;
    }
    (current_param, &tokens[i..], use_self)
}

fn gen_widget_name(path: &Path) -> String {
    let name = path_to_string(path);
    let name =
        if let Some(index) = name.rfind(':') {
            name[index + 1 ..].to_lowercase()
        }
        else {
            name.to_lowercase()
        };
    let mut hashmap = NAMES_INDEX.lock().expect("lock() in gen_widget_name()");
    let index = hashmap.entry(name.clone()).or_insert(0);
    *index += 1;
    format!("{}{}", name, index)
}

fn path_to_string(path: &Path) -> String {
    let mut string = String::new();
    for segment in &path.segments {
        string.push_str(segment.ident.as_ref());
    }
    string
}

fn parse_attributes(mut tokens: &[TokenTree]) -> (HashMap<&str, Option<&str>>, &[TokenTree]) {
    let mut attributes = HashMap::new();
    while tokens[0] == Token(Pound) {
        tokens = &tokens[1..];
        if let TokenTree::Delimited(Delimited { delim: Bracket, ref tts }) = tokens[0] {
            tokens = &tokens[1..];
            if let Token(Ident(ref ident)) = tts[0] {
                let name = ident.as_ref();
                let value =
                    if let Some(&Token(Eq)) = tts.get(1) {
                        if let Token(Literal(Str(ref name, Cooked))) = tts[2] {
                            Some(name.as_str())
                        }
                        else {
                            None
                        }
                    }
                    else {
                        None
                    };
                attributes.insert(name, value);
            }
        }
    }
    (attributes, tokens)
}

fn parse_child_properties(mut tokens: &[TokenTree]) -> HashMap<String, Tokens> {
    // TODO: panic if the same child properties is set twice.
    // TODO: same for normal properties?
    let mut properties = HashMap::new();
    while !tokens.is_empty() {
        let (ident, _) = parse_ident(tokens);
        tokens = &tokens[1..];
        if let Token(Colon) = tokens[0] {
            tokens = &tokens[1..];
            let (value, new_tokens, _) = parse_value(tokens, NotEvent);
            tokens = new_tokens;
            properties.insert(ident, value);
        }

        if tokens.first() == Some(&Token(Comma)) {
            tokens = &tokens[1..];
        }
    }
    properties
}

fn parse_relm_widget(tokens: &[TokenTree]) -> (Widget, &[TokenTree]) {
    let (relm_type, mut tokens) = parse_qualified_name(tokens);
    let mut relm_widget = RelmWidget::new();
    let mut init_parameters = vec![];
    let mut children = vec![];
    let mut properties = HashMap::new();
    let mut child_properties = HashMap::new();
    let mut child_events = HashMap::new();
    if let TokenTree::Delimited(Delimited { delim: Paren, ref tts }) = tokens[0] {
        let parameters = parse_comma_list(tts);
        init_parameters = parameters;
        tokens = &tokens[1..];
    }
    if let TokenTree::Delimited(Delimited { delim: Brace, ref tts }) = tokens[0] {
        let mut tts = &tts[..];
        while !tts.is_empty() {
            let is_child =
                if let Some((_, next_tokens)) = try_parse_name(tts) {
                    if let TokenTree::Delimited(Delimited { delim: Brace, .. }) = next_tokens[0] {
                        true
                    }
                    else {
                        false
                    }
                }
                else {
                    false
                };
            if tts[0] == Token(Pound) || is_child {
                let (child, new_tts, _) = parse_child(tts, false);
                tts = new_tts;
                children.push(child);
            }
            else {
                // Property or event.
                let (ident, _) = parse_ident(tts);
                tts = &tts[1..];
                match tts[0] {
                    Token(Colon) => {
                        tts = parse_value_or_child_properties(tts, ident, &mut child_properties, &mut properties);
                    },
                    Token(Dot) => {
                        let child_name = ident;
                        let (ident, new_tts) = parse_ident(&tts[1..]);
                        let (event, new_tts) = parse_event(new_tts, DefaultOneParam);
                        child_events.insert((child_name, ident), event);
                        tts = new_tts;
                    },
                    TokenTree::Delimited(Delimited { delim: Paren, .. }) | Token(FatArrow) => {
                        if ident.chars().next().map(|char| char.is_lowercase()) == Some(false) {
                            // Uppercase is a msg.
                            let (event, new_tts) = parse_event(&tts[0..], DefaultNoParam);
                            let mut entry = relm_widget.events.entry(ident).or_insert_with(Vec::new);
                            entry.push(event);
                            tts = new_tts;
                        }
                        else {
                            // Lowercase is a gtk event.
                            let (event, new_tts) = parse_event(tts, DefaultOneParam);
                            relm_widget.gtk_events.insert(ident, event);
                            tts = new_tts;
                        }
                    },
                    _ => panic!("Expected `:`, `=>` or `(` but found `{:?}` in view! macro", tts[0]),
                }
            }

            if tts.first() == Some(&Token(Comma)) {
                tts = &tts[1..];
            }
        }
    }
    let widget = Widget::new_relm(relm_widget, relm_type, init_parameters, children, properties, child_properties,
                                  child_events);
    (widget, &tokens[1..])
}

fn try_parse_async(tokens: &[TokenTree]) -> (bool, &[TokenTree]) {
    if let Token(Pound) = tokens[0] {
        if let TokenTree::Delimited(Delimited { delim: Bracket, ref tts }) = tokens[1] {
            if tts[0] == Token(Ident(syn::Ident::new("async"))) {
                return (true, &tokens[2..]);
            }
        }
    }
    (false, tokens)
}

fn tokens_to_expr(tokens: Tokens) -> Expr {
    let string: String = tokens.parse().expect("parse::<String>() in tokens_to_expr");
    parse_expr(&string).expect("parse_expr in tokens_to_expr")
}
