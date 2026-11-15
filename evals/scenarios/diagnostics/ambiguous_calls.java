//- /app/Ambiguous.java
package app;

import java.util.concurrent.Callable;
import java.util.function.Consumer;
import java.util.function.Function;

class Ambiguous {
    interface A { String apply(String x); }
    interface B { String apply(String x); }
    static String choose(A f) { return f.apply("x"); }
    static String choose(B f) { return f.apply("x"); }

    static void pair(int a, long b) {}
    static void pair(long a, int b) {}

    static void named(String s) {}
    static void named(Integer i) {}

    static void widen(Object o) {}
    static void widen(String s) {}

    static void num(int i) {}
    static void num(long l) {}

    static void run(Runnable r) {}
    static <T> void run(Callable<T> c) {}

    static void each(Consumer<String> c) {}
    static void each(Function<String, Integer> f) {}

    static int answer() { return 42; }

    void calls() {
        /*@lambda*/choose(x -> x.trim());
        /*@prims*/pair(1, 1);
        /*@null*/named(null);
        // 表达式体是方法调用：两个函数类型都潜在兼容
        /*@each*/each(s -> s.length());

        // 下面这些都有唯一最具体的候选
        widen("s");
        widen(null);
        num(1);
        pair(1, 1L);
        run(() -> answer());
        each(s -> { System.out.println(s); });
        each(s -> s.length() + 1);
    }
}
//- /app/StaticConflict.java
package app;

import static java.lang.Math.abs;
import static java.lang.StrictMath.abs;

class StaticConflict {
    int f() { return /*@abs*/abs(1); }
}
//- /app/StaticMerged.java
package app;

import static app.StaticMerged.Ints.twice;
import static app.StaticMerged.Strings.twice;

class StaticMerged {
    static class Ints { static int twice(int x) { return x * 2; } }
    static class Strings { static String twice(String s) { return s + s; } }

    void f() {
        int a = twice(2);
        String b = twice("b");
    }
}
//- /app/GenericVsPlain.java
package app;

import java.io.IOException;
import java.util.Iterator;

class GenericVsPlain {
    <A extends Appendable> A appendTo(A appendable, Iterator<?> parts) throws IOException {
        return appendable;
    }

    StringBuilder appendTo(StringBuilder builder, Iterator<?> parts) {
        return builder;
    }

    // 非泛型的重载更具体，选中它就不会抛 IOException
    StringBuilder use(StringBuilder builder, Iterable<?> parts) {
        return appendTo(builder, parts.iterator());
    }
}
//? diag ambiguous-call lambda
//? diag ambiguous-call prims
//? diag ambiguous-call null
//? diag ambiguous-call abs
//? diag ambiguous-call each
//? diag exact
