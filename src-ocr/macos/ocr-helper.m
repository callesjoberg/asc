#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <Vision/Vision.h>

int main(int argc, const char * argv[]) {
    @autoreleasepool {
        if (argc < 2) {
            printf("Error: Missing image path argument.\n");
            return 1;
        }
        
        NSString *imagePath = [NSString stringWithUTF8String:argv[1]];
        NSURL *fileURL = [NSURL fileURLWithPath:imagePath];
        
        NSImage *image = [[NSImage alloc] initWithContentsOfURL:fileURL];
        if (!image) {
            printf("Error: Could not load image from %s\n", argv[1]);
            return 1;
        }
        
        CGImageRef cgImage = [image CGImageForProposedRect:NULL context:nil hints:nil];
        if (!cgImage) {
            printf("Error: Could not get CGImage from NSImage\n");
            return 1;
        }
        
        VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:cgImage options:@{}];
        
        VNRecognizeTextRequest *request = [[VNRecognizeTextRequest alloc] initWithCompletionHandler:^(VNRequest * _Nonnull request, NSError * _Nullable error) {
            if (error) {
                printf("OCR Error: %s\n", [[error localizedDescription] UTF8String]);
                exit(1);
            }
            
            NSArray *results = request.results;
            BOOL wordsJSON = argc > 2 && strcmp(argv[2], "--words-json") == 0;
            NSMutableArray *words = [NSMutableArray array];
            for (VNRecognizedTextObservation *observation in results) {
                NSArray<VNRecognizedText *> *topCandidates = [observation topCandidates:1];
                if (topCandidates.count > 0) {
                    VNRecognizedText *candidate = topCandidates[0];
                    if (wordsJSON) {
                        NSString *text = candidate.string;
                        [text enumerateSubstringsInRange:NSMakeRange(0, text.length)
                                                 options:NSStringEnumerationByWords
                                              usingBlock:^(NSString *substring, NSRange substringRange, NSRange enclosingRange, BOOL *stop) {
                            NSError *boxError = nil;
                            VNRectangleObservation *box = [candidate boundingBoxForRange:substringRange error:&boxError];
                            if (!box || boxError) return;
                            CGRect bounds = box.boundingBox;
                            double imageWidth = CGImageGetWidth(cgImage);
                            double imageHeight = CGImageGetHeight(cgImage);
                            [words addObject:@{
                                @"text": substring,
                                @"x": @(bounds.origin.x * imageWidth),
                                @"y": @((1.0 - bounds.origin.y - bounds.size.height) * imageHeight),
                                @"width": @(bounds.size.width * imageWidth),
                                @"height": @(bounds.size.height * imageHeight)
                            }];
                        }];
                    } else {
                        printf("%s\n", [candidate.string UTF8String]);
                    }
                }
            }
            if (wordsJSON) {
                NSData *json = [NSJSONSerialization dataWithJSONObject:words options:0 error:nil];
                printf("%s\n", [[[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding] UTF8String]);
            }
        }];
        
        request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
        request.usesLanguageCorrection = YES;
        request.recognitionLanguages = @[@"sv-SE", @"en-US"];
        
        NSError *error = nil;
        [handler performRequests:@[request] error:&error];
        if (error) {
            printf("Error performing OCR request: %s\n", [[error localizedDescription] UTF8String]);
            return 1;
        }
    }
    return 0;
}
