import java.util.List;

class Box</*@t*/T extends Comparable<T>> {
    private /*@use_t*/T /*@item*/item;

    /*@use_t2*/T /*@get*/get() { return /*@use_item*/item; }

    <R> R /*@convert*/convert(java.util.function.Function<T, /*@use_r*/R> f) { return f.apply(item); }

    static </*@e*/E extends Comparable<E>> /*@use_e*/E /*@max*/max(List<E> xs) { return xs.get(0); }
}

class Main {
    int run(Box<String> box, List<Integer> numbers) {
        String s = box./*@call_get*/get();
        Integer n = box./*@call_convert*/convert(String::length);
        return s./*@length*/length() + n + Box./*@call_max*/max(numbers);
    }
}
//? def use_t -> t
//? def use_t2 -> t
//? def use_item -> item
//? def use_e -> e
//? def call_get -> get
//? def call_convert -> convert
//? def call_max -> max
//? def length -> lib java.lang.String#length
//? diag none
