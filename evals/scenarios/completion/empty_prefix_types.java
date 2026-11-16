package app;

import java.util.List;
import org.missing.annotations.Nullable;
import org.missing.annotations.Immutable;

class Helper {}

class Main {
    @Nullable String cached;

    @/*@ann*/
    public String toString() { return ""; }

    @Imm/*@unresolved*/
    static class Frozen {}

    @Nu/*@unresolved_used*/
    String other;

    void run(/*@param*/ value, List<String> names) {
        String first = names.get(0);
        Helper helper = new Helper();
        System.out.println(first + helper);
    }

    void stmt() {
        /*@expr*/.out.println();
    }
}
//? complete ann has Override SuppressWarnings Deprecated FunctionalInterface Nullable
//? complete ann lacks String Helper
//? complete unresolved has Immutable
//? complete unresolved_used has Nullable
//? complete param has String Helper List Object
//? complete param order String Object
//? complete expr has System
