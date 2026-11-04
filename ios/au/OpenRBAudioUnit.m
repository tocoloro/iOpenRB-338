#import <AVFoundation/AVFoundation.h>
#import <AudioToolbox/AudioToolbox.h>

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

@implementation OpenRBAudioUnit {
    OrbAu *_au;
    AUAudioUnitBusArray *_outputs;
    double _rate;
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
    AVAudioFormat *format = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:_rate channels:2];
    AUAudioUnitBus *bus = [[AUAudioUnitBus alloc] initWithFormat:format error:outError];
    if (bus == nil) {
        return nil;
    }
    bus.maximumChannelCount = 2;
    _outputs = [[AUAudioUnitBusArray alloc] initWithAudioUnit:self
                                                      busType:AUAudioUnitBusTypeOutput
                                                       busses:@[ bus ]];
    self.maximumFramesToRender = 4096;
    return self;
}

- (void)dealloc {
    orb_au_free(_au);
    _au = NULL;
}

- (AUAudioUnitBusArray *)outputBusses {
    return _outputs;
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
        double beat = 0.0;
        int playing = 0;
        AUHostTransportStateBlock transport = self_.transportStateBlock;
        AUHostMusicalContextBlock musical = self_.musicalContextBlock;
        if (transport != nil && musical != nil) {
            AUHostTransportStateFlags flags = 0;
            if (transport(&flags, NULL, NULL, NULL)) {
                playing = (flags & AUHostTransportStateMoving) != 0;
            }
            musical(&tempo, NULL, NULL, &beat, NULL, NULL);
        }
        double rate = self_->_rate;
        if (outputData->mNumberBuffers >= 2) {
            float *left = outputData->mBuffers[0].mData;
            float *right = outputData->mBuffers[1].mData;
            if (left != NULL && right != NULL) {
                orb_au_render(au, left, right, (uint32_t)frameCount, rate, tempo, beat, playing);
            }
            return noErr;
        }
        float *interleaved = outputData->mBuffers[0].mData;
        if (interleaved == NULL) {
            return noErr;
        }
        uint32_t frames = (uint32_t)frameCount;
        if (frames > 4096) {
            frames = 4096;
        }
        float left[4096];
        float right[4096];
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
