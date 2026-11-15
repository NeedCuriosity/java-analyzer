//- /app/Shapes.java
package app;

import java.util.List;
import java.util.function.BiFunction;

public /*@sealed*/sealed interface Shapes /*@permits*/permits Circle, Square {
    /*@priv*/private int helper() { return 1; }
    default int area() { return helper(); }
}

final class Circle implements Shapes {}

final class Square implements Shapes {}

/*@rec*/record Point(int x, int y) {}

class Uses {
    String text = /*@tb*/"""
        hello
        """;

    int run(Object o, int day, List<String> names) {
        /*@var*/var count = 0;
        for (/*@forvar*/var name : names) {
            count += name.length();
        }
        BiFunction<Integer, Integer, Integer> add = (/*@lvar*/var a, /*@lvar2*/var b) -> a + b;
        if (o instanceof /*@pat*/String s) {
            count += s.length();
        }
        int size = /*@swexpr*/switch (day) {
            case 1 /*@arrow*/-> 10;
            default /*@arrow2*/-> {
                /*@yield*/yield 20;
            }
        };
        int _ = 3;
        return count + size + add.apply(1, 2);
    }
}
//? level 8
//? diag feature-not-available sealed
//? diag feature-not-available permits
//? diag feature-not-available priv
//? diag feature-not-available rec
//? diag feature-not-available tb
//? diag feature-not-available var
//? diag feature-not-available forvar
//? diag feature-not-available lvar
//? diag feature-not-available pat
//? diag feature-not-available swexpr
//? diag feature-not-available arrow
//? diag feature-not-available arrow2
//? diag feature-not-available lvar2
//? diag feature-not-available yield
//? diag only feature-not-available
