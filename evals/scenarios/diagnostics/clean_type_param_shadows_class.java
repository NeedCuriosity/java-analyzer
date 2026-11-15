import java.util.function.Function;

class Policy {}

class Name {}

class Box<Name> {
    Name content;

    <Policy> Policy convert(Function<Name, Policy> mapper) {
        return mapper.apply(content);
    }

    static <Policy> Policy make(Function<String, Policy> mapper) {
        return mapper.apply("abc");
    }

    static <Name> Name first(java.util.List<Name> items) {
        return items.get(0);
    }
}

class Main {
    void run(Box<String> box, java.util.List<Long> numbers) {
        Integer length = box./*@convert*/convert(text -> text.length());
        Integer made = Box./*@make*/make(text -> text.length());
        Long first = Box.first(numbers);
    }
}
//? diag none
//? hover convert ~ "<Policy> Policy convert(Function<Name, Policy> mapper)"
//? hover make ~ "<Policy> Policy make(Function<String, Policy> mapper)"
