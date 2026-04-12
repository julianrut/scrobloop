// system_audio.m — ScreenCaptureKit system audio capture for Scrobloop
// Requires macOS 12.3+. Called from Rust via C FFI.

#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <CoreMedia/CoreMedia.h>
#import <AudioToolbox/AudioToolbox.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// ─── Minimal WAV writer ───────────────────────────────────────────────────────

static void put16(FILE *f, uint16_t v) {
    fputc((uint8_t)(v),      f);
    fputc((uint8_t)(v >> 8), f);
}
static void put32(FILE *f, uint32_t v) {
    fputc((uint8_t)(v),       f);
    fputc((uint8_t)(v >>  8), f);
    fputc((uint8_t)(v >> 16), f);
    fputc((uint8_t)(v >> 24), f);
}

static FILE *wav_open(const char *path, uint32_t rate, uint16_t ch) {
    FILE *f = fopen(path, "wb");
    if (!f) return NULL;
    fwrite("RIFF", 1, 4, f); put32(f, 0);       // size patched on close
    fwrite("WAVE", 1, 4, f);
    fwrite("fmt ", 1, 4, f); put32(f, 16);
    put16(f, 1);                                 // PCM
    put16(f, ch);
    put32(f, rate);
    put32(f, rate * ch * 2);                     // byte rate
    put16(f, (uint16_t)(ch * 2));               // block align
    put16(f, 16);                               // bits per sample
    fwrite("data", 1, 4, f); put32(f, 0);       // data size patched on close
    return f;
}

static void wav_finalize(FILE *f, uint32_t data_bytes) {
    fseek(f,  4, SEEK_SET); put32(f, 36 + data_bytes);
    fseek(f, 40, SEEK_SET); put32(f, data_bytes);
    fclose(f);
}

// Write interleaved int16 from non-interleaved float32 channels
static uint32_t wav_write_noninterleaved(FILE *f,
                                          AudioBufferList *abl,
                                          uint32_t num_frames,
                                          uint16_t ch) {
    uint32_t bytes_written = 0;
    for (uint32_t frame = 0; frame < num_frames; frame++) {
        for (uint16_t c = 0; c < ch && c < abl->mNumberBuffers; c++) {
            float *data = (float *)abl->mBuffers[c].mData;
            float s = data[frame];
            if (s >  1.f) s =  1.f;
            if (s < -1.f) s = -1.f;
            int16_t v = (int16_t)(s * 32767.f);
            fputc((uint8_t)(v),       f);
            fputc((uint8_t)(v >> 8),  f);
            bytes_written += 2;
        }
    }
    return bytes_written;
}

// Write interleaved int16 from interleaved float32
static uint32_t wav_write_interleaved(FILE *f, const float *data, size_t count) {
    for (size_t i = 0; i < count; i++) {
        float s = data[i];
        if (s >  1.f) s =  1.f;
        if (s < -1.f) s = -1.f;
        int16_t v = (int16_t)(s * 32767.f);
        fputc((uint8_t)(v),       f);
        fputc((uint8_t)(v >> 8),  f);
    }
    return (uint32_t)(count * 2);
}

// ─── SCStream delegate ────────────────────────────────────────────────────────

@interface SysAudioCapture : NSObject <SCStreamOutput, SCStreamDelegate>
- (instancetype)initWithPath:(const char *)path
                    stopFlag:(const volatile uint8_t *)flag
                     maxSecs:(uint64_t)secs;
- (int)start;   // 0 = ok, -1 = permission denied, -2 = other error
- (void)waitUntilDone;
@end

@implementation SysAudioCapture {
    NSString              *_path;
    const volatile uint8_t *_stopFlag;
    uint64_t               _maxSecs;
    SCStream              *_stream;
    FILE                  *_wav;
    uint32_t               _dataBytes;
    NSDate                *_startTime;
    uint16_t               _channels;
    BOOL                   _wavOpen;
}

- (instancetype)initWithPath:(const char *)path
                    stopFlag:(const volatile uint8_t *)flag
                     maxSecs:(uint64_t)secs {
    if (!(self = [super init])) return nil;
    _path     = [NSString stringWithUTF8String:path];
    _stopFlag = flag;
    _maxSecs  = secs;
    _wavOpen  = NO;
    _dataBytes = 0;
    return self;
}

- (int)start {
    __block SCShareableContent *content = nil;
    __block NSError *contentErr = nil;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);

    [SCShareableContent
        getShareableContentExcludingDesktopWindows:NO
        onScreenWindowsOnly:NO
        completionHandler:^(SCShareableContent *c, NSError *e) {
            content    = c;
            contentErr = e;
            dispatch_semaphore_signal(sem);
        }];
    dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC));

    if (contentErr || content.displays.count == 0) {
        // Permission denied or no display
        return -1;
    }

    SCDisplay *display = content.displays.firstObject;
    SCContentFilter *filter = [[SCContentFilter alloc]
        initWithDisplay:display excludingWindows:@[]];

    SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
    cfg.capturesAudio            = YES;
    cfg.excludesCurrentProcessAudio = NO;
    cfg.sampleRate               = 44100;
    cfg.channelCount             = 2;
    // Capture a tiny 2×2 video frame to keep CPU overhead minimal
    cfg.width                    = 2;
    cfg.height                   = 2;
    cfg.minimumFrameInterval     = CMTimeMake(1, 1); // 1 fps

    _stream = [[SCStream alloc]
        initWithFilter:filter
        configuration:cfg
        delegate:self];

    NSError *addErr = nil;
    BOOL ok = [_stream
        addStreamOutput:self
        type:SCStreamOutputTypeAudio
        sampleHandlerQueue:dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0)
        error:&addErr];
    if (!ok) return -2;

    __block NSError *startErr = nil;
    dispatch_semaphore_t startSem = dispatch_semaphore_create(0);
    [_stream startCaptureWithCompletionHandler:^(NSError *e) {
        startErr = e;
        dispatch_semaphore_signal(startSem);
    }];
    dispatch_semaphore_wait(startSem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC));

    if (startErr) return -1; // most likely a permission error

    _startTime = [NSDate date];
    return 0;
}

- (void)waitUntilDone {
    while (YES) {
        if (*_stopFlag) break;
        if (-[_startTime timeIntervalSinceNow] >= (NSTimeInterval)_maxSecs) break;
        [NSThread sleepForTimeInterval:0.05];
    }

    dispatch_semaphore_t stopSem = dispatch_semaphore_create(0);
    [_stream stopCaptureWithCompletionHandler:^(NSError *e) {
        dispatch_semaphore_signal(stopSem);
    }];
    dispatch_semaphore_wait(stopSem, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC));

    if (_wav) {
        wav_finalize(_wav, _dataBytes);
        _wav = NULL;
    }
}

// ─── SCStreamOutput ───────────────────────────────────────────────────────────

- (void)stream:(SCStream *)stream
    didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
    ofType:(SCStreamOutputType)type {

    if (type != SCStreamOutputTypeAudio) return;
    if (*_stopFlag) return;

    CMFormatDescriptionRef fmt = CMSampleBufferGetFormatDescription(sampleBuffer);
    if (!fmt) return;
    const AudioStreamBasicDescription *asbd =
        CMAudioFormatDescriptionGetStreamBasicDescription(fmt);
    if (!asbd) return;

    // Open WAV file on first sample so we know the actual rate/channels
    if (!_wavOpen) {
        _channels = (uint16_t)asbd->mChannelsPerFrame;
        _wav = wav_open([_path UTF8String],
                        (uint32_t)asbd->mSampleRate,
                        _channels);
        _wavOpen = YES;
    }
    if (!_wav) return;

    // Two-call pattern to get correctly sized AudioBufferList
    size_t ablSize = 0;
    CMBlockBufferRef blockBuf = NULL;
    CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sampleBuffer, &ablSize, NULL, 0,
        kCFAllocatorDefault, kCFAllocatorDefault, 0, NULL);

    if (ablSize == 0) return;
    AudioBufferList *abl = (AudioBufferList *)malloc(ablSize);
    OSStatus status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sampleBuffer, &ablSize, abl, ablSize,
        kCFAllocatorDefault, kCFAllocatorDefault,
        kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
        &blockBuf);

    if (status == noErr && abl->mNumberBuffers > 0) {
        CMItemCount numFrames = CMSampleBufferGetNumSamples(sampleBuffer);
        BOOL nonInterleaved = (asbd->mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0;

        if (nonInterleaved) {
            _dataBytes += wav_write_noninterleaved(_wav, abl, (uint32_t)numFrames, _channels);
        } else {
            size_t count = abl->mBuffers[0].mDataByteSize / sizeof(float);
            _dataBytes += wav_write_interleaved(_wav, (float *)abl->mBuffers[0].mData, count);
        }
    }

    free(abl);
    if (blockBuf) CFRelease(blockBuf);
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    // Stream stopped unexpectedly; waitUntilDone will timeout and finalize
}

@end

// ─── C entry point called from Rust ──────────────────────────────────────────

// Returns:  0 = success
//          -1 = permission denied (prompt user to enable Screen Recording)
//          -2 = other error
int capture_system_audio(const char *path,
                         uint64_t max_secs,
                         const volatile uint8_t *stop_flag) {
    @autoreleasepool {
        SysAudioCapture *cap = [[SysAudioCapture alloc]
            initWithPath:path stopFlag:stop_flag maxSecs:max_secs];
        int rc = [cap start];
        if (rc != 0) return rc;
        [cap waitUntilDone];
        return 0;
    }
}
