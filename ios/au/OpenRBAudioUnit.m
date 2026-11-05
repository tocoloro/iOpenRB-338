#import <AVFoundation/AVFoundation.h>
#import <AudioToolbox/AudioToolbox.h>
#import <CoreAudioKit/CoreAudioKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>
#import <math.h>
#import <stdlib.h>

// The engine, built by crates/orb-au. The host supplies the transport.
// The principal class is an AUViewController. On iOS that is a real class;
// AUViewControllerBase is only a typedef of UIViewController, and a host
// rejects the panel with paramErr (-50) when the entry point is that.
typedef struct OrbAu OrbAu;
extern OrbAu *orb_au_new(void);
extern void orb_au_free(OrbAu *au);
extern void orb_au_render(OrbAu *au, float *left, float *right, uint32_t frames,
                          double host_rate, double tempo_bpm, double beat, int playing);
// The panel. The main thread is the only caller. `layer` is a CAMetalLayer.
extern void orb_au_ui_frame(OrbAu *au, void *layer, float width_pts, float height_pts, float scale,
                            float bottom_inset);
extern void orb_au_ui_touch(OrbAu *au, uint64_t touch_id, int phase, float x, float y);
extern void orb_au_ui_detach(OrbAu *au);

@interface OpenRBAudioUnit : AUAudioUnit
- (OrbAu *)engine;
@end

// Principal class of the extension, and the panel AUM shows.
@interface OpenRBViewController : AUViewController <AUAudioUnitFactory>
@end

enum { kOpenRBPullFrames = 4096 };

@implementation OpenRBAudioUnit {
    OrbAu *_au;
    AUAudioUnitBusArray *_inputs;
    AUAudioUnitBusArray *_outputs;
    double _rate;
    // AUM sets these just before allocateRenderResources and asks the plugin to
    // keep the copies. The render callback uses these copies.
    AUHostTransportStateBlock _transport;
    AUHostMusicalContextBlock _musical;
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

// The iPad's graph is 48 kHz. Refusing that leaves the unit unconnected.
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
    _transport = self.transportStateBlock;
    _musical = self.musicalContextBlock;
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
        double sample_pos = 0.0;
        AUHostTransportStateBlock transport = self_->_transport;
        AUHostMusicalContextBlock musical = self_->_musical;
        // No transport block: a host that never publishes one still makes sound.
        // A host that does publish one stays silent until it reports moving.
        int playing = transport == nil ? 1 : 0;
        if (transport != nil) {
            AUHostTransportStateFlags flags = 0;
            double cycle_start = 0.0;
            double cycle_end = 0.0;
            if (transport(&flags, &sample_pos, &cycle_start, &cycle_end)) {
                playing = (flags & AUHostTransportStateMoving) != 0;
            }
        }
        if (musical != nil) {
            double reported_tempo = 0.0;
            double numerator = 4.0;
            NSInteger denominator = 4;
            double reported_beat = 0.0;
            NSInteger next_beat = 0;
            double downbeat = 0.0;
            if (musical(&reported_tempo, &numerator, &denominator, &reported_beat, &next_beat,
                        &downbeat)) {
                tempo = reported_tempo;
                beat = reported_beat;
            }
        }
        if (playing && !isfinite(beat) && isfinite(sample_pos) && isfinite(tempo) && tempo > 0.0 &&
            self_->_rate > 0.0) {
            beat = sample_pos * tempo / 60.0 / self_->_rate;
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

- (OrbAu *)engine {
    return _au;
}

- (void)requestViewControllerWithCompletionHandler:(void (^)(AUViewControllerBase *__nullable))handler {
    // The extension machinery packages the panel for AUM. Passing the object
    // from here is what comes back as paramErr (-50).
    [super requestViewControllerWithCompletionHandler:handler];
}

@end

// The panel AUM shows when the plugin is tapped. It is the same panel as the app.
// The host already owns UIApplication, so this view draws into its own metal layer.

@interface OpenRBMetalView : UIView
@property (nonatomic, assign) OrbAu *au;
@end

@implementation OpenRBMetalView

+ (Class)layerClass {
    return [CAMetalLayer class];
}

- (instancetype)initWithFrame:(CGRect)frame {
    self = [super initWithFrame:frame];
    if (self == nil) {
        return nil;
    }
    self.multipleTouchEnabled = YES;
    self.opaque = YES;
    self.backgroundColor = [UIColor colorWithRed:0x1d / 255.0 green:0x21 / 255.0 blue:0x27 / 255.0 alpha:1];
    CAMetalLayer *layer = (CAMetalLayer *)self.layer;
    layer.pixelFormat = MTLPixelFormatBGRA8Unorm;
    layer.framebufferOnly = YES;
    layer.opaque = YES;
    return self;
}

- (void)sendTouches:(NSSet<UITouch *> *)touches {
    if (self.au == NULL) {
        return;
    }
    for (UITouch *touch in touches) {
        int phase = 0;
        switch (touch.phase) {
        case UITouchPhaseBegan:
            phase = 0;
            break;
        case UITouchPhaseMoved:
        case UITouchPhaseStationary:
            phase = 1;
            break;
        case UITouchPhaseEnded:
            phase = 2;
            break;
        case UITouchPhaseCancelled:
            phase = 3;
            break;
        default:
            continue;
        }
        CGPoint p = [touch locationInView:self];
        orb_au_ui_touch(self.au, (uint64_t)(__bridge void *)touch, phase, (float)p.x, (float)p.y);
    }
}

- (void)touchesBegan:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    (void)event;
    [self sendTouches:touches];
}

- (void)touchesMoved:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    (void)event;
    [self sendTouches:touches];
}

- (void)touchesEnded:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    (void)event;
    [self sendTouches:touches];
}

- (void)touchesCancelled:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    (void)event;
    [self sendTouches:touches];
}

@end

@interface OpenRBViewController ()
@property (nonatomic, strong) OpenRBAudioUnit *unit;
@property (nonatomic, strong) OpenRBMetalView *metal;
@property (nonatomic, strong) CADisplayLink *link;
@end

@implementation OpenRBViewController

- (instancetype)initWithNibName:(NSString *)name bundle:(NSBundle *)bundle {
    self = [super initWithNibName:name bundle:bundle];
    if (self == nil) {
        return nil;
    }
    // A zero size is an empty window: the tap looks like it did nothing.
    self.preferredContentSize = CGSizeMake(800, 640);
    return self;
}

- (void)beginRequestWithExtensionContext:(NSExtensionContext *)context {
    [super beginRequestWithExtensionContext:context];
}

- (AUAudioUnit *)createAudioUnitWithComponentDescription:(AudioComponentDescription)desc
                                                   error:(NSError **)error {
    OpenRBAudioUnit *unit = [[OpenRBAudioUnit alloc] initWithComponentDescription:desc
                                                                          options:0
                                                                            error:error];
    if (unit == nil) {
        return nil;
    }
    self.unit = unit;
    [self attachEngine];
    return unit;
}

- (void)attachEngine {
    if (self.metal != nil && self.unit != nil) {
        self.metal.au = [self.unit engine];
    }
}

- (void)viewDidLoad {
    [super viewDidLoad];
    self.view.backgroundColor = [UIColor colorWithRed:0x1d / 255.0 green:0x21 / 255.0 blue:0x27 / 255.0 alpha:1];
    OpenRBMetalView *metal = [[OpenRBMetalView alloc] initWithFrame:self.view.bounds];
    metal.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    [self.view addSubview:metal];
    self.metal = metal;
    [self attachEngine];
    self.link = [CADisplayLink displayLinkWithTarget:self selector:@selector(tick:)];
    [self.link addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
}

- (void)tick:(CADisplayLink *)link {
    (void)link;
    OpenRBMetalView *metal = self.metal;
    if (metal == nil || metal.au == NULL) {
        return;
    }
    CGSize size = metal.bounds.size;
    CGFloat scale = metal.contentScaleFactor;
    if (scale < 1) {
        scale = 1;
    }
    CAMetalLayer *layer = (CAMetalLayer *)metal.layer;
    layer.contentsScale = scale;
    layer.drawableSize = CGSizeMake(size.width * scale, size.height * scale);
    UIEdgeInsets insets = self.view.safeAreaInsets;
    orb_au_ui_frame(metal.au, (__bridge void *)layer, (float)size.width, (float)size.height, (float)scale,
                    (float)insets.bottom);
}

- (void)viewWillAppear:(BOOL)animated {
    [super viewWillAppear:animated];
    if (self.link == nil) {
        self.link = [CADisplayLink displayLinkWithTarget:self selector:@selector(tick:)];
        [self.link addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
    }
}

- (void)viewWillDisappear:(BOOL)animated {
    [super viewWillDisappear:animated];
    // The display link retains this controller. Invalidating it here is what lets the
    // controller go away when AUM closes the panel.
    [self.link invalidate];
    self.link = nil;
    if (self.metal.au != NULL) {
        orb_au_ui_detach(self.metal.au);
    }
}

- (void)dealloc {
    [_link invalidate];
    if (_metal.au != NULL) {
        orb_au_ui_detach(_metal.au);
    }
}

@end
