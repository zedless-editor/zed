#![allow(unused, dead_code)]

use gpui::Global;
use std::ops::{Deref, DerefMut};
use ui::prelude::*;

/// Debug only: Used for testing various account states
///
/// Use this by initializing it with
/// `cx.set_global(DebugAccountState::default());` somewhere
///
/// Then call `cx.debug_account()` to get access
#[derive(Clone, Debug)]
pub struct DebugAccountState {
    pub enabled: bool,
}

impl DebugAccountState {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) -> &mut Self {
        self.enabled = enabled;
        self
    }
}

impl Default for DebugAccountState {
    fn default() -> Self {
        Self {
            enabled: false,
        }
    }
}

impl DebugAccountState {
    pub fn get_global(cx: &App) -> &Self {
        &cx.global::<GlobalDebugAccountState>().0
    }
}

#[derive(Clone, Debug)]
pub struct GlobalDebugAccountState(pub DebugAccountState);

impl Global for GlobalDebugAccountState {}

impl Deref for GlobalDebugAccountState {
    type Target = DebugAccountState;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for GlobalDebugAccountState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub trait DebugAccount {
    fn debug_account(&self) -> &DebugAccountState;
}

impl DebugAccount for App {
    fn debug_account(&self) -> &DebugAccountState {
        &self.global::<GlobalDebugAccountState>().0
    }
}
