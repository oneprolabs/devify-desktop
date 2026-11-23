// Explicit build mode: a normal Vite preview is not an installed development app.
export const DEVELOPMENT_APP = import.meta.env?.MODE === 'development-app';
export const APP_NAME = DEVELOPMENT_APP ? 'AIMyChats Dev' : 'AIMyChats';
export const APP_IDENTIFIER = DEVELOPMENT_APP ? 'local.wechat.devifybridge.dev' : 'local.wechat.devifybridge';
