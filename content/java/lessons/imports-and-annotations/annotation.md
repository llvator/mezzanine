---
id: annotation-fundamentals
language: java
applies-to: [annotation]
title: "Annotations — metadata attached to declarations"
level: beginner
sources:
  - "Java_L Module 14 — Annotations (AnnotationBasics, BuiltInAnnotations)"
  - "JLS §9.6 — Annotation Types"
  - "JLS §9.7 — Annotations"
---

# Annotations — metadata attached to declarations

An *annotation* is a label written with a leading `@` that attaches metadata to a declaration — a class, method, field, parameter, or another annotation. The compiler, the JVM, or a framework reads that metadata and acts on it; the annotation itself is inert.

## Syntax

```java
import java.util.List;
import javax.annotation.Resource;

public class Order {

    @Resource                              // framework injects this field at startup
    private OrderRepository repository;

    @Deprecated                            // marker — no arguments
    public void oldApi() { }

    @Override                              // marker, checked by the compiler
    public String toString() {
        return "Order";
    }

    @SuppressWarnings("unchecked")         // annotation with arguments
    public List<String> raw() {
        return (List<String>) (List<?>) List.of();
    }
}
```

## Key ideas

**Annotations don't change semantics by themselves.** `@Override` doesn't override anything — the method below it does. The annotation only asks the compiler to verify that the marked method actually overrides something in a supertype; remove the annotation and the method still overrides correctly. The same holds for `@Deprecated`, `@SuppressWarnings`, `@FunctionalInterface` — each is a hint to a *reader of the metadata*, not a runtime behaviour.

**Marker vs argument forms.** A *marker* annotation has no parentheses (`@Override`, `@Deprecated`). An annotation with arguments names key-value pairs in parentheses (`@SuppressWarnings("unchecked")`, `@Resource(name = "ds")`). A single-element annotation can elide the `value =` key (`@SuppressWarnings({"unchecked", "raw"})`). Both forms are structurally the same — they instantiate an annotation type at compile time.

**Retention decides who sees them.** Each annotation type declares `@Retention(SOURCE | CLASS | RUNTIME)`. Source-retained annotations (`@Override`, `@SuppressWarnings`) disappear after the compiler reads them. Class-retained ones stay in the `.class` file but aren't visible to the running JVM. Runtime-retained ones (`@Deprecated`, most framework annotations like `@Resource`, `@Autowired`, `@Test`) are visible via reflection — which is how Spring, JUnit, Hibernate, and JEE find the declarations they care about.

**Framework annotations are the dominant use.** Most annotations a working Java program carries belong to a framework: dependency injection (`@Inject`, `@Resource`, `@Autowired`), persistence (`@Entity`, `@Column`), serialization (`@JsonProperty`), web routing (`@GetMapping`), testing (`@Test`, `@BeforeEach`). The annotation is a static contract; a startup-time scanner or a code generator does the actual work.

**You can write your own.** An annotation type is declared with `@interface Name { … }`. Inside the braces go the elements (each looks like an abstract method with an optional `default`). Custom annotations are useful when paired with an annotation processor or a reflection-based framework — never write one whose only consumer is `if (m.isAnnotationPresent(X.class))` inside the same module that declared it; a plain method or interface is clearer.

## Related

- Rule: [`prefer-constructor-injection`](../../rules/imports-and-annotations/prefer-constructor-injection.md) — fires on `@Resource` / `@Autowired` / `@Inject` to suggest constructor injection over field injection.
- Lesson: [`field-declaration`](../oop/field-declaration.md) — the declaration that framework annotations like `@Resource` most commonly attach to.
- Lesson: [`method-declaration`](../oop/method-declaration.md) — `@Override` and `@Deprecated` live on these.
