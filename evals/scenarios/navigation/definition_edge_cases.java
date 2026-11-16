import java.util.ArrayList;
import java.util.List;
import java.util.function.Function;
import java.util.function.Supplier;

class /*@animalDecl*/Animal {
    /*@animalCtor*/Animal() {}
    /*@animalCtorName*/Animal(String name) {}
    String /*@speakDecl*/speak() { return "..."; }
}

class /*@dogDecl*/Dog extends Animal {
    /*@dogCtor*/Dog() { /*@superCtor*/super("dog"); }
    /*@dogCtor2*/Dog(int n) { /*@thisCtor*/this(); }

    @Override
    String speak() { return /*@superSpeak*/super./*@superSpeakName*/speak() + "!"; }

    static Supplier<Dog> factory() { return Dog::/*@newRef*/new; }

    Function<String, Animal> named() { return Animal::/*@newRefArg*/new; }

    void use() {
        /*@varKw*/var list = new ArrayList<String>();
        List<Dog> dogs = List.of(new /*@newDog*/Dog());
        dogs.forEach(/*@lambdaParam*/d -> /*@lambdaUse*/d.speak());
        Animal a = new /*@newAnimalStr*/Animal("x");
        String s = /*@thisSpeak*/this.speak();
        list.add(s + a);
        Object o = a;
        if (o instanceof Dog /*@patDecl*/dog && /*@patUse*/dog.speak() != null) {
            list.add(dog.toString());
        }
    }
}
//? def superCtor -> animalCtorName
//? def thisCtor -> dogCtor
//? def superSpeakName -> speakDecl
//? def newRef -> dogCtor
//? def newRefArg -> animalCtorName
//? def newDog -> dogCtor
//? def lambdaUse -> lambdaParam
//? def newAnimalStr -> animalCtorName
//? def patUse -> patDecl
//? typedef varKw -> lib java.util.ArrayList
//? diag none
//? def varKw -> lib java.util.ArrayList
//? hover varKw ~ "ArrayList"
