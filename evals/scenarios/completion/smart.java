import java.util.List;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import java.util.function.Function;

class Main {
    void f(List<String> names) {
        List<String> a = new /*@n1*/;
        Map<String, Integer> m = new /*@n2*/;
        Runnable r = /*@lam*/;
        Function<String, Integer> fn = /*@lam2*/;
        TimeUnit unit = /*@en*/;
        names.stream().map(/*@arg*/);
        sleep(/*@en2*/);
        Shape s = new /*@impl*/;
    }

    void sleep(TimeUnit u) {}
}

interface Shape {}

class Circle implements Shape {}
//? complete n1 first ArrayList
//? insert n1 ArrayList ~ "ArrayList<>(" snippet
//? complete n1 order ArrayList LinkedList List
//? complete n2 first HashMap
//? complete lam first "() -> {}"
//? complete lam2 first "s -> "
//? insert lam2 "s -> " ~ "s -> $0" snippet
//? complete en order TimeUnit.SECONDS names
//? complete en lacks unit
//? complete arg first "s -> "
//? complete en2 has TimeUnit.DAYS
//? complete impl first Circle
