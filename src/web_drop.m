#import <AppKit/AppKit.h>

typedef void (*DDReceiveURL)(void *, const char *);
typedef void (*DDRelease)(void *);

// GPUI registers the window for Finder paths. This ancestor of its rendering
// view receives browser URL drags while leaving mouse and keyboard targeting
// and the window's existing file-drop path in place.
@interface DDWebDropView : NSView <NSDraggingDestination>
@property(nonatomic) void *context;
@property(nonatomic) DDReceiveURL receiveURL;
@property(nonatomic) DDRelease releaseContext;
@end

@implementation DDWebDropView

- (void)dealloc {
    if (_releaseContext) _releaseContext(_context);
}

- (NSString *)webURL:(id<NSDraggingInfo>)drag {
    NSPasteboard *board = drag.draggingPasteboard;
    NSString *value = [board stringForType:NSPasteboardTypeURL];
    if (!value) value = [board stringForType:NSPasteboardTypeString];
    value = [value stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    if (!value || [value rangeOfCharacterFromSet:NSCharacterSet.whitespaceAndNewlineCharacterSet].location != NSNotFound) return nil;
    NSURL *url = [NSURL URLWithString:value];
    NSString *scheme = url.scheme.lowercaseString;
    if (url.host.length && ([scheme isEqual:@"https"] || [scheme isEqual:@"http"])) return value;
    return nil;
}

- (NSDragOperation)webOperation:(id<NSDraggingInfo>)drag {
    NSDragOperation allowed = drag.draggingSourceOperationMask;
    if (allowed & NSDragOperationCopy) return NSDragOperationCopy;
    if (allowed & NSDragOperationLink) return NSDragOperationLink;
    return allowed & NSDragOperationGeneric;
}

- (NSDragOperation)draggingEntered:(id<NSDraggingInfo>)drag {
    return [self webURL:drag] ? [self webOperation:drag] : [(id<NSDraggingDestination>)self.window draggingEntered:drag];
}

- (NSDragOperation)draggingUpdated:(id<NSDraggingInfo>)drag {
    return [self webURL:drag] ? [self webOperation:drag] : [(id<NSDraggingDestination>)self.window draggingUpdated:drag];
}

- (void)draggingExited:(id<NSDraggingInfo>)drag {
    if (![self webURL:drag]) [(id<NSDraggingDestination>)self.window draggingExited:drag];
}

- (BOOL)performDragOperation:(id<NSDraggingInfo>)drag {
    NSString *url = [self webURL:drag];
    if (!url) return [(id<NSDraggingDestination>)self.window performDragOperation:drag];
    _receiveURL(_context, url.UTF8String);
    return YES;
}

- (void)concludeDragOperation:(id<NSDraggingInfo>)drag {
    if (![self webURL:drag]) [(id<NSDraggingDestination>)self.window concludeDragOperation:drag];
}

@end

void dd_install_web_drop(void *view, void *context, DDReceiveURL receive, DDRelease release) {
    NSView *child = (__bridge NSView *)view;
    DDWebDropView *host = [[DDWebDropView alloc] initWithFrame:child.frame];
    host.context = context;
    host.receiveURL = receive;
    host.releaseContext = release;
    host.autoresizingMask = child.autoresizingMask;
    [host registerForDraggedTypes:@[NSPasteboardTypeURL, NSPasteboardTypeString]];
    [child.superview replaceSubview:child with:host];
    child.frame = host.bounds;
    [host addSubview:child];
}
