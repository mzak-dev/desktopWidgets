# Declarative animation, because of the redraw scheduler

Windows redraw only on a changed binding, an in-flight animation, or input, so an idle desktop costs zero frames. That only holds if the engine can ask "is anything animating?". Animations are therefore declared (property, duration, easing) and clocked by the engine; arbitrary imperative animation would force defensive redraws.
