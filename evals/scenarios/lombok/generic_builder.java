//- /lombok/Builder.java
package lombok;
public @interface Builder { String builderMethodName() default "builder"; String builderClassName() default ""; boolean toBuilder() default false; }
//- /lombok/Singular.java
package lombok;
public @interface Singular { String value() default ""; }
//- /app/Pair.java
package app;

import java.util.List;
import lombok.Builder;
import lombok.Singular;

@Builder(toBuilder = true)
public class Pair<A, B extends Comparable<B>> {
    private A first;
    private B second;
    @Singular
    private List<A> items;

    @Builder(builderMethodName = "of", builderClassName = "OfBuilder")
    static <X> Pair<X, Integer> make(X x, int y) {
        return null;
    }
}
//- /app/Box.java
package app;

import lombok.Builder;

@Builder
public record Box<T>(T value, String label) {}
//- /app/Main.java
package app;

class Main {
    void run() {
        Pair<String, Integer> p = Pair.<String, Integer>builder().first("x").second(1).item("y").build();
        var /*@q*/q = Pair.<String, Integer>builder().first("x").build();
        Pair.PairBuilder<String, Integer> target = Pair.builder();
        target.second(2);
        var /*@copy*/copy = p.toBuilder();
        var /*@made*/made = Pair.<String>of().x("s").y(2).build();
        Box<String> box = Box.<String>builder().value("v").label("l").build();
        var /*@v*/v = box.value();
    }
}
//? hover q ~ "Pair<String, Integer> q"
//? hover copy ~ "PairBuilder<String, Integer> copy"
//? hover made ~ "Pair<String, Integer> made"
//? hover v ~ "String v"
//? diag exact
