#import <Foundation/Foundation.h>
#include <stdbool.h>
#include <stdio.h>

bool loofah_coordinate(const char *path, bool writing, void *context,
                       void (*accessor)(void *), char *error, size_t capacity) {
    @autoreleasepool {
        NSURL *url = [NSURL fileURLWithFileSystemRepresentation:path isDirectory:YES relativeToURL:nil];
        NSFileCoordinator *coordinator = [[NSFileCoordinator alloc] initWithFilePresenter:nil];
        __block bool called = false;
        NSError *failure = nil;
        if (writing) {
            [coordinator coordinateWritingItemAtURL:url options:0 error:&failure byAccessor:^(NSURL *newURL) {
                if (![newURL.URLByStandardizingPath.path isEqualToString:url.URLByStandardizingPath.path]) { return; }
                called = true;
                accessor(context);
            }];
        } else {
            [coordinator coordinateReadingItemAtURL:url options:0 error:&failure byAccessor:^(NSURL *newURL) {
                if (![newURL.URLByStandardizingPath.path isEqualToString:url.URLByStandardizingPath.path]) { return; }
                called = true;
                accessor(context);
            }];
        }
        if (failure || !called) {
            snprintf(error, capacity, "%s", failure.localizedDescription.UTF8String ?: "File coordination cancelled");
            return false;
        }
        return true;
    }
}

bool loofah_versions(const char *path, void *context,
                     void (*version)(void *, const char *, const char *), char *error, size_t capacity) {
    (void)error;
    (void)capacity;
    @autoreleasepool {
        NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:path]];
        NSArray<NSFileVersion *> *versions = [NSFileVersion unresolvedConflictVersionsOfItemAtURL:url];
        for (NSFileVersion *item in versions) {
            if (item.URL.fileSystemRepresentation) {
                NSData *identifier = [NSKeyedArchiver archivedDataWithRootObject:item.persistentIdentifier requiringSecureCoding:NO error:NULL];
                NSString *token = [identifier base64EncodedStringWithOptions:0];
                if (token) { version(context, item.URL.fileSystemRepresentation, token.UTF8String); }
            }
        }
        return true;
    }
}

void loofah_resolve_version(const char *path, const char *versionToken) {
    @autoreleasepool {
        NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:path]];
        NSString *target = [NSString stringWithUTF8String:versionToken];
        for (NSFileVersion *version in [NSFileVersion unresolvedConflictVersionsOfItemAtURL:url]) {
            NSData *identifier = [NSKeyedArchiver archivedDataWithRootObject:version.persistentIdentifier requiringSecureCoding:NO error:NULL];
            if ([[identifier base64EncodedStringWithOptions:0] isEqualToString:target]) { version.resolved = YES; }
        }
    }
}

int loofah_materialize(const char *path, char *error, size_t capacity) {
    @autoreleasepool {
        NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:path]];
        NSNumber *ubiquitous = nil;
        [url getResourceValue:&ubiquitous forKey:NSURLIsUbiquitousItemKey error:NULL];
        NSString *placeholder = [url.URLByDeletingLastPathComponent.path stringByAppendingPathComponent:
            [NSString stringWithFormat:@".%@.icloud", url.lastPathComponent]];
        if (!ubiquitous.boolValue && ![[NSFileManager defaultManager] fileExistsAtPath:placeholder]) { return 0; }
        NSString *status = nil;
        [url getResourceValue:&status forKey:NSURLUbiquitousItemDownloadingStatusKey error:NULL];
        if ([status isEqualToString:NSURLUbiquitousItemDownloadingStatusCurrent]) { return 0; }
        NSError *failure = nil;
        if ([[NSFileManager defaultManager] startDownloadingUbiquitousItemAtURL:url error:&failure]) { return 1; }
        snprintf(error, capacity, "%s", failure.localizedDescription.UTF8String ?: "iCloud item is unavailable");
        return -1;
    }
}
