package app;

import java.util./*@imp*/;

class Main {
    void unimported() {
        ArrayLis/*@unimported*/
    }

    void lang() {
        StringBu/*@lang*/
    }

    Object afterNew() {
        return new HashM/*@after_new*/;
    }

    void fieldType(java.util.concurrent.atomic.Atom/*@qualified*/ counter) {}

    @Overr/*@annotation*/
    public String toString() { return ""; }
}
//? complete imp has List Map ArrayList concurrent
//? complete imp lacks String
//? complete unimported has ArrayList
//? complete lang has StringBuilder StringBuffer
//? complete after_new has HashMap
//? complete qualified has AtomicInteger AtomicLong
//? complete annotation has Override
