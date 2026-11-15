//- /lib/util/Strings.java
package lib.util;

public class /*@strings*/Strings {
    public static final String /*@empty*/EMPTY = "";
    public static String /*@pad*/pad(String s) { return s; }
    public static class /*@builder*/Builder { }
}
//- /lib/model/Point.java
package lib.model;

public class /*@point*/Point {
    public int /*@x*/x;
}
//- /app/Main.java
package app;

import lib.util.Strings;
import lib.util.Strings.Builder;
import lib.model.*;
import static lib.util.Strings.pad;
import static lib.util.Strings.*;

class Main {
    String run(/*@use_point*/Point p, /*@use_builder*/Builder b) {
        lib.model./*@qualified_point*/Point q = p;
        return /*@use_strings*/Strings.pad(/*@use_empty*/EMPTY) + /*@use_pad*/pad("x") + q./*@use_x*/x;
    }
}
//? def use_point -> point
//? def use_builder -> builder
//? def qualified_point -> point
//? def use_strings -> strings
//? def use_empty -> empty
//? def use_pad -> pad
//? def use_x -> x
//? diag none
