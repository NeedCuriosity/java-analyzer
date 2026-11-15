import java.math.BigDecimal;
import java.time.LocalDate;
import java.util.List;
import java.util.Optional;

class Main {
    void strings(String s, StringBuilder sb) {
        s./*@trim*/trim();
        s./*@replace*/replace('a', 'b');
        String./*@join*/join(",", List.of());
        sb.append(s);
        s.getChars(0, 1, new char[1], 0);
        String t = s.trim();
        System.out.println(t);
    }

    void values(BigDecimal amount, LocalDate date, Optional<String> name) {
        amount./*@scale*/setScale(2);
        date./*@plus*/plusDays(1);
        name.orElseThrow();
        name.get();
        name./*@map*/map(String::length);
        Integer.parseInt("12");
        amount.intValueExact();
        date.toString();
        Character.toChars(0x1F600, new char[2], 0);
    }

    int arms(String s, int k) {
        switch (k) {
            case 1 -> s./*@stmt_arm*/strip();
            default -> System.out.println(s);
        }
        return switch (k) {
            case 1 -> s.length();
            default -> s.indexOf('x');
        };
    }

    void streams(List<String> items) {
        items.stream()./*@filter*/filter(s -> !s.isEmpty());
        items.stream().forEach(System.out::println);
        items.stream().count();
        items.stream().collect(java.util.stream.Collectors.toList());
        items.stream()./*@parallel*/parallel();
        items.removeIf(String::isEmpty);
    }
}
//? diag ignored-result trim
//? diag ignored-result replace
//? diag ignored-result join
//? diag ignored-result scale
//? diag ignored-result plus
//? diag ignored-result map
//? diag ignored-result filter
//? diag ignored-result parallel
//? diag ignored-result stmt_arm
//? diag only ignored-result
