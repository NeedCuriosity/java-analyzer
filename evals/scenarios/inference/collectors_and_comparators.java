import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.TreeMap;
import java.util.function.Function;
import java.util.stream.Collectors;
import java.util.stream.Stream;

class Person {
    String name;
    int age;
    String getName() { return name; }
    int getAge() { return age; }
}

class Main {
    void run(List<String> names, List<Person> people, Map<String, List<Integer>> index) {
        var /*@tomap*/tomap = names.stream().collect(Collectors.toMap(s -> s, s -> s.length()));
        var /*@tomap2*/tomap2 = names.stream().collect(Collectors.toMap(Function.identity(), String::length));
        var /*@tree*/tree = names.stream().collect(Collectors.toMap(s -> s, String::length, (a, b) -> a, TreeMap::new));
        var /*@byage*/byage = people.stream().collect(Collectors.groupingBy(Person::getAge, Collectors.mapping(Person::getName, Collectors.toSet())));
        var /*@counts*/counts = people.stream().collect(Collectors.groupingBy(p -> p.getName(), Collectors.counting()));
        var /*@parts*/parts = names.stream().collect(Collectors.partitioningBy(s -> s.isEmpty()));
        var /*@cmp*/cmp = Comparator.comparing(Person::getName).thenComparing(Person::getAge);
        var /*@cmp2*/cmp2 = Comparator.comparing((Person p) -> p.getAge()).reversed();
        var /*@cmp3*/cmp3 = Comparator.comparing(Person::getName, Comparator.reverseOrder());
        var /*@cmp4*/cmp4 = Comparator.<Person>comparingInt(p -> p.age).thenComparing(p -> p.name);
        var /*@sorted*/sorted = people.stream().sorted(Comparator.comparing(Person::getAge)).map(Person::getName).toList();
        var /*@flat*/flat = index.values().stream().flatMap(List::stream).collect(Collectors.toSet());
        var /*@entries*/entries = index.entrySet().stream().map(e -> e.getKey() + e.getValue().size()).toList();
        var /*@reduced*/reduced = Stream.of(1, 2, 3).reduce(0, Integer::sum);
        var /*@maxby*/maxby = people.stream().max(Comparator.comparingInt(Person::getAge)).map(p -> p.name);
        var /*@nested*/nested = Optional.of(names).map(l -> l.get(0)).orElse("");
        var /*@avg*/avg = people.stream().collect(Collectors.averagingInt(Person::getAge));
        var /*@summary*/summary = people.stream().mapToInt(Person::getAge).summaryStatistics();
        var /*@grouped2*/grouped2 = people.stream().collect(Collectors.groupingBy(Person::getAge, TreeMap::new, Collectors.toList()));
        var /*@joined*/joined = people.stream().map(Person::getName).collect(Collectors.joining(", ", "[", "]"));
        var /*@coll*/coll = people.stream().collect(Collectors.collectingAndThen(Collectors.toList(), List::size));
        Set<String> /*@target*/target = people.stream().map(p -> p.name).collect(Collectors.toSet());
        System.out.println("" + tomap + tomap2 + tree + byage + counts + parts + cmp + cmp2 + cmp3 + cmp4
                + sorted + flat + entries + reduced + maxby + nested + avg + summary + grouped2 + joined + coll + target);
    }
}
//? hover tomap ~ "Map<String, Integer> tomap"
//? hover tomap2 ~ "Map<String, Integer> tomap2"
//? hover tree ~ "TreeMap<String, Integer> tree"
//? hover byage ~ "Map<Integer, Set<String>> byage"
//? hover counts ~ "Map<String, Long> counts"
//? hover parts ~ "Map<Boolean, List<String>> parts"
//? hover cmp ~ "Comparator<Person> cmp"
//? hover cmp2 ~ "Comparator<Person> cmp2"
//? hover cmp3 ~ "Comparator<Person> cmp3"
//? hover cmp4 ~ "Comparator<Person> cmp4"
//? hover sorted ~ "List<String> sorted"
//? hover flat ~ "Set<Integer> flat"
//? hover entries ~ "List<String> entries"
//? hover reduced ~ "Integer reduced"
//? hover maxby ~ "Optional<String> maxby"
//? hover nested ~ "String nested"
//? hover avg ~ "Double avg"
//? hover summary ~ "IntSummaryStatistics summary"
//? hover grouped2 ~ "TreeMap<Integer, List<Person>> grouped2"
//? hover joined ~ "String joined"
//? hover coll ~ "Integer coll"
//? diag none
