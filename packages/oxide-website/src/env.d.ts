/// <reference types="vite/client" />

/** CSS side-effect imports — the bundler hashes the stylesheet via Lightning
 *  CSS and injects the <link>; TS needs only to know that importing a .css
 *  file is a valid side-effect. */
declare module '*.css' {}
