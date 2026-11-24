// Loaded into a native Switch game that Orivo starts (DYLD_INSERT_LIBRARIES,
// see nxnative::console_env): the game's fullscreen covers the screen on the
// desktop the player is already on, instead of the Space of its own that
// macOS fullscreen makes. Orivo is itself a fullscreen window on that desktop,
// so going from Orivo to the game switches no Space, as on a console.
//
// It works on any nxnative Mac app, whoever packed it (nxnative's own
// scripts, NX2Apple, NX Play): every one asks for fullscreen the same way,
// through -[NSWindow toggleFullScreen:] on the game window (front_mac.m), and
// none is built with the hardened runtime, since the game's code is mapped
// executable at run time. Only the window changes: no game code is touched.
//
// - The game window covers its screen as soon as it is shown when the game
//   starts in fullscreen (DM_FULLSCREEN): no title bar, the menu bar and the
//   Dock hidden while the game is the active app. The game asks for
//   fullscreen itself only once it is the active app, which a game started
//   from another app is not always at once; the player should not see a
//   window first.
// - toggleFullScreen: from the game's own startup (no sender) covers; from
//   the Window menu or ⌃⌘F (a menu item) it covers, or gives the window back
//   as it was.
// - A window without a title bar takes the keyboard only when it says it can,
//   so a covering window says so.
// - A screen that changes size is covered again.

#import <Cocoa/Cocoa.h>
#import <objc/runtime.h>

static NSMapTable<NSWindow *, NSDictionary *> *g_covering;   // window → its style and frame before

static BOOL covers(NSWindow *window) { return [g_covering objectForKey:window] != nil; }

static void cover(NSWindow *window) {
    NSScreen *screen = window.screen ?: NSScreen.mainScreen;
    if (!screen) return;
    [g_covering setObject:@{@"style": @(window.styleMask), @"frame": [NSValue valueWithRect:window.frame], @"background": window.backgroundColor} forKey:window];
    window.styleMask = NSWindowStyleMaskBorderless;
    // Black, not the window's white, while the game loads its first frame:
    // the screen goes dark as a console's does.
    window.backgroundColor = NSColor.blackColor;
    window.contentView.layer.backgroundColor = NSColor.blackColor.CGColor;
    [window setFrame:screen.frame display:YES];
    // AppKit applies these only while this app is the active one, so the
    // player's menu bar and Dock are back as soon as they leave the game.
    NSApp.presentationOptions = NSApplicationPresentationHideDock | NSApplicationPresentationHideMenuBar;
    [window makeKeyAndOrderFront:nil];
}

static void uncover(NSWindow *window) {
    NSDictionary *before = [g_covering objectForKey:window];
    [g_covering removeObjectForKey:window];
    window.styleMask = [before[@"style"] unsignedIntegerValue];
    window.backgroundColor = before[@"background"];
    [window setFrame:[before[@"frame"] rectValue] display:YES];
    NSApp.presentationOptions = NSApplicationPresentationDefault;
    [window makeKeyAndOrderFront:nil];
}

static void (*g_toggle)(id, SEL, id);
static void (*g_order_front)(id, SEL, id);
static BOOL (*g_can_key)(id, SEL);
static BOOL (*g_can_main)(id, SEL);

static void orivo_toggle(NSWindow *self, SEL _cmd, id sender) {
    (void)_cmd; (void)sender;
    if (!NSThread.isMainThread) {
        dispatch_async(dispatch_get_main_queue(), ^{ orivo_toggle(self, _cmd, sender); });
        return;
    }
    if (!covers(self)) cover(self);
    else if (sender) uncover(self);   // the player's request; the game's startup asks with none
}

/// Fullscreen at start, as DM_FULLSCREEN asks (any value but 0).
static BOOL starts_full(void) {
    const char *full = getenv("DM_FULLSCREEN");
    return full && strcmp(full, "0");
}

static void orivo_order_front(NSWindow *self, SEL _cmd, id sender) {
    g_order_front(self, _cmd, sender);
    static BOOL done;
    if (done || covers(self) || !(self.collectionBehavior & NSWindowCollectionBehaviorFullScreenPrimary) || !starts_full()) return;
    done = YES;   // once: a window the player gave back stays a window
    cover(self);
}

static BOOL orivo_can_key(NSWindow *self, SEL _cmd) { return covers(self) || g_can_key(self, _cmd); }
static BOOL orivo_can_main(NSWindow *self, SEL _cmd) { return covers(self) || g_can_main(self, _cmd); }

static void replace(SEL selector, IMP with, IMP *original) {
    Method method = class_getInstanceMethod(NSWindow.class, selector);
    if (!method) return;
    *original = method_setImplementation(method, with);
}

__attribute__((constructor)) static void orivo_fill_start(void) {
    g_covering = [NSMapTable weakToStrongObjectsMapTable];
    replace(@selector(toggleFullScreen:), (IMP)orivo_toggle, (IMP *)&g_toggle);
    replace(@selector(makeKeyAndOrderFront:), (IMP)orivo_order_front, (IMP *)&g_order_front);
    replace(@selector(canBecomeKeyWindow), (IMP)orivo_can_key, (IMP *)&g_can_key);
    replace(@selector(canBecomeMainWindow), (IMP)orivo_can_main, (IMP *)&g_can_main);
    [NSNotificationCenter.defaultCenter addObserverForName:NSApplicationDidChangeScreenParametersNotification object:nil
                                                     queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *note) {
        (void)note;
        for (NSWindow *window in g_covering.keyEnumerator)
            if (window.screen) [window setFrame:window.screen.frame display:YES];
    }];
}
