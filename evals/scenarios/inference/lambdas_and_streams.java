import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.function.Function;
import java.util.stream.Collectors;

class Main {
    void run(List<String> names, Map<String, Integer> ages) {
        var /*@lengths*/lengths = names.stream().map(String::length).collect(Collectors.toList());
        var /*@upper*/upper = names.stream().map(s -> s.toUpperCase()).toList();
        var /*@any*/any = names.stream().filter(s -> !s.isEmpty()).findFirst();
        var /*@count*/count = names.stream().count();
        var /*@joined*/joined = names.stream().collect(Collectors.joining(","));
        var /*@opt*/opt = Optional.of("x").map(String::length);
        var /*@grouped*/grouped = names.stream().collect(Collectors.groupingBy(String::length));
        Function<String, Integer> f = /*@p*/p -> /*@use_p*/p.length();
        names.forEach(/*@n*/n -> System.out.println(n));
        ages.forEach((/*@key*/key, /*@age*/age) -> System.out.println(key + age));
        var /*@sum*/sum = names.stream().mapToInt(String::length).sum();
    }
}
//? hover lengths ~ "List<Integer> lengths"
//? hover upper ~ "List<String> upper"
//? hover any ~ "Optional<String> any"
//? hover count ~ "long count"
//? hover joined ~ "String joined"
//? hover opt ~ "Optional<Integer> opt"
//? hover grouped ~ "Map<Integer, List<String>> grouped"
//? hover p ~ "String p"
//? hover n ~ "String n"
//? hover key ~ "String key"
//? hover age ~ "Integer age"
//? hover sum ~ "int sum"
//? diag none
