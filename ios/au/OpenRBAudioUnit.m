#import <AVFoundation/AVFoundation.h>
#import <AudioToolbox/AudioToolbox.h>
#import <math.h>
#import <stdlib.h>

// The engine, built by crates/orb-au. No view and no note input: the host
// supplies the transport, and the pattern follows that.
typedef struct OrbAu OrbAu;
extern OrbAu *orb_au_new(void);
extern void orb_au_free(OrbAu *au);
extern void orb_au_render(OrbAu *au, float *left, float *right, uint32_t frames,
                          double host_rate, double tempo_bpm, double beat, int playing);

@interface OpenRBAudioUnit : AUAudioUnit
@end

@interface OpenRBAudioUnitFactory : NSObject <AUAudioUnitFactory>
@end

enum { kOpenRBPullFrames = 4096 };

@implementation OpenRBAudioUnit {
    OrbAu *_au;
    AUAudioUnitBusArray *_inputs;
    AUAudioUnitBusArray *_outputs;
    double _rate;
    float *_pullLeft;
    float *_pullRight;
    float *_pullInterleaved;
}

- (instancetype)initWithComponentDescription:(AudioComponentDescription)componentDescription
                                     options:(AudioComponentInstantiationOptions)options
                                       error:(NSError **)outError {
    self = [super initWithComponentDescription:componentDescription options:options error:outError];
    if (self == nil) {
        return nil;
    }
    _au = orb_au_new();
    _rate = 44100.0;
    _pullLeft = calloc(kOpenRBPullFrames, sizeof(float));
    _pullRight = calloc(kOpenRBPullFrames, sizeof(float));
    _pullInterleaved = calloc(kOpenRBPullFrames * 2, sizeof(float));
    AVAudioFormat *format = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:_rate channels:2];
    AUAudioUnitBus *bus = [[AUAudioUnitBus alloc] initWithFormat:format error:outError];
    if (bus == nil) {
        return nil;
    }
    bus.maximumChannelCount = 2;
    _inputs = [[AUAudioUnitBusArray alloc] initWithAudioUnit:self
                                                     busType:AUAudioUnitBusTypeInput
                                                      busses:@[]];
    _outputs = [[AUAudioUnitBusArray alloc] initWithAudioUnit:self
                                                      busType:AUAudioUnitBusTypeOutput
                                                       busses:@[ bus ]];
    self.maximumFramesToRender = kOpenRBPullFrames;
    return self;
}

- (void)dealloc {
    orb_au_free(_au);
    _au = NULL;
    free(_pullLeft);
    free(_pullRight);
    free(_pullInterleaved);
}

- (AUAudioUnitBusArray *)inputBusses {
    return _inputs;
}

- (AUAudioUnitBusArray *)outputBusses {
    return _outputs;
}

// The iPad's graph is 48 kHz. Refusing that leaves the unit unconnected, which
// AUM shows as a red cross and then plays silence.
- (BOOL)shouldChangeToFormat:(AVAudioFormat *)format forBus:(AUAudioUnitBus *)bus {
    (void)bus;
    if (format == nil || format.commonFormat != AVAudioPCMFormatFloat32) {
        return NO;
    }
    if (format.channelCount != 2) {
        return NO;
    }
    double rate = format.sampleRate;
    return rate >= 8000.0 && rate <= 192000.0;
}

- (BOOL)allocateRenderResourcesAndReturnError:(NSError **)outError {
    if (![super allocateRenderResourcesAndReturnError:outError]) {
        return NO;
    }
    double rate = self.outputBusses[0].format.sampleRate;
    _rate = rate >= 8000.0 ? rate : 44100.0;
    return YES;
}

- (AUInternalRenderBlock)internalRenderBlock {
    OrbAu *au = _au;
    __unsafe_unretained OpenRBAudioUnit *self_ = self;
    return ^AUAudioUnitStatus(AudioUnitRenderActionFlags *actionFlags, const AudioTimeStamp *timestamp,
                              AVAudioFrameCount frameCount, NSInteger outputBusNumber,
                              AudioBufferList *outputData, const AURenderEvent *realtimeEventListHead,
                              AURenderPullInputBlock pullInputBlock) {
        (void)actionFlags;
        (void)timestamp;
        (void)outputBusNumber;
        (void)realtimeEventListHead;
        (void)pullInputBlock;
        if (outputData == NULL || outputData->mNumberBuffers < 1 || frameCount == 0) {
            return noErr;
        }
        double tempo = 0.0;
        // Not a number: the host gave no beat, so the engine keeps its own clock.
        double beat = NAN;
        int playing = 1;
        AUHostTransportStateBlock transport = self_.transportStateBlock;
        AUHostMusicalContextBlock musical = self_.musicalContextBlock;
        if (transport != nil) {
            AUHostTransportStateFlags flags = 0;
            if (transport(&flags, NULL, NULL, NULL)) {
                playing = (flags & AUHostTransportStateMoving) != 0;
            }
        }
        if (musical != nil) {
            double reported_tempo = 0.0;
            double reported_beat = 0.0;
            if (musical(&reported_tempo, NULL, NULL, &reported_beat, NULL, NULL)) {
                tempo = reported_tempo;
                beat = reported_beat;
            }
        }
        double rate = self_->_rate;
        uint32_t frames = (uint32_t)frameCount;
        if (frames > kOpenRBPullFrames) {
            frames = kOpenRBPullFrames;
        }
        if (outputData->mNumberBuffers >= 2) {
            float *left = outputData->mBuffers[0].mData;
            float *right = outputData->mBuffers[1].mData;
            if (left == NULL) {
                left = self_->_pullLeft;
                outputData->mBuffers[0].mData = left;
                outputData->mBuffers[0].mDataByteSize = frames * (UInt32)sizeof(float);
                outputData->mBuffers[0].mNumberChannels = 1;
            }
            if (right == NULL) {
                right = self_->_pullRight;
                outputData->mBuffers[1].mData = right;
                outputData->mBuffers[1].mDataByteSize = frames * (UInt32)sizeof(float);
                outputData->mBuffers[1].mNumberChannels = 1;
            }
            if (left != NULL && right != NULL) {
                orb_au_render(au, left, right, frames, rate, tempo, beat, playing);
            }
            return noErr;
        }
        float *interleaved = outputData->mBuffers[0].mData;
        if (interleaved == NULL) {
            interleaved = self_->_pullInterleaved;
            outputData->mBuffers[0].mData = interleaved;
            outputData->mBuffers[0].mDataByteSize = frames * 2 * (UInt32)sizeof(float);
            outputData->mBuffers[0].mNumberChannels = 2;
        }
        float *left = self_->_pullLeft;
        float *right = self_->_pullRight;
        orb_au_render(au, left, right, frames, rate, tempo, beat, playing);
        for (uint32_t i = 0; i < frames; i++) {
            interleaved[2 * i] = left[i];
            interleaved[2 * i + 1] = right[i];
        }
        return noErr;
    };
}

@end

@implementation OpenRBAudioUnitFactory

- (void)beginRequestWithExtensionContext:(NSExtensionContext *)context {
    (void)context;
}

- (AUAudioUnit *)createAudioUnitWithComponentDescription:(AudioComponentDescription)desc
                                                   error:(NSError **)error {
    return [[OpenRBAudioUnit alloc] initWithComponentDescription:desc options:0 error:error];
}

@end
